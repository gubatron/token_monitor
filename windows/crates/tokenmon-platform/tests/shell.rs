//! Hermetic tests for the shell integrations, run against in-memory fakes.

use std::sync::Arc;

use tokenmon_platform::shell::autostart::{RUN_KEY, STARTUP_APPROVED_KEY, VALUE_NAME};
use tokenmon_platform::shell::command_line::{self, executable};
use tokenmon_platform::shell::context_menu::{COMMAND_KEY, VERB_KEY};
use tokenmon_platform::shell::ipc::{MAX_MESSAGE_LEN, pipe_name};
use tokenmon_platform::shell::{
    DesktopVerb, FakeInstanceLock, FakeIpc, InstanceLock, InstanceState, IpcMessage, IpcParseError,
    LaunchArgs, LoginItemService, LoginItemStatus, MemoryRegistry, Registry, RunKeyLoginItem,
    ShellError, ShellVerbService, StartupDecision, decide_startup,
};

const EXE: &str = r"C:\Program Files\TokenMon\TokenMon.exe";

fn login(reg: &Arc<MemoryRegistry>, exe: &str) -> RunKeyLoginItem {
    RunKeyLoginItem::new(reg.clone(), exe)
}

fn verb(reg: &Arc<MemoryRegistry>) -> DesktopVerb {
    DesktopVerb::new(reg.clone(), EXE)
}

// ---- command lines -------------------------------------------------------

/// Paths with spaces are quoted so Windows does not split at the space.
#[test]
fn test_command_quotes_path_with_spaces() {
    assert_eq!(
        command_line::build(EXE, &["--background"]).unwrap(),
        r#""C:\Program Files\TokenMon\TokenMon.exe" --background"#
    );
}

/// Paths that cannot be quoted safely are rejected, never escaped.
#[test]
fn test_command_rejects_unusable_paths() {
    for bad in ["", "C:\\a\"b.exe", "C:\\a\nb.exe", "C:\\a\0b.exe", "a\rb"] {
        assert!(
            matches!(
                command_line::quote_exe(bad),
                Err(ShellError::InvalidExecutablePath(_))
            ),
            "{bad:?}"
        );
    }
}

/// `executable` recovers what `build` put in, including unusual but legal paths.
#[test]
fn test_command_executable_roundtrips() {
    for exe in [
        EXE,
        r"C:\tools\a b\c&d.exe",
        r"\\server\share\tm.exe",
        "tm.exe",
    ] {
        let line = command_line::build(exe, &["--show"]).unwrap();
        assert_eq!(executable(&line), Some(exe));
    }
}

/// Unquoted legacy values and malformed ones are handled without panicking.
#[test]
fn test_command_executable_handles_odd_input() {
    assert_eq!(
        executable(r"C:\x\tm.exe --background"),
        Some(r"C:\x\tm.exe")
    );
    assert_eq!(executable("   "), None);
    assert_eq!(executable(""), None);
    assert_eq!(executable("\"unterminated"), None);
    assert_eq!(executable("\"\" --x"), None);
}

// ---- CLI -----------------------------------------------------------------

/// Known flags parse, case-insensitively and in any order.
#[test]
fn test_cli_parses_known_flags() {
    let a = LaunchArgs::parse(["--Show", "--BACKGROUND"]).unwrap();
    assert!(a.show && a.background);
    assert_eq!(
        LaunchArgs::parse(Vec::<String>::new()).unwrap(),
        LaunchArgs::default()
    );
}

/// Unknown flags are a typed error naming the argument; lenient mode skips them.
#[test]
fn test_cli_rejects_unknown_but_lenient_ignores() {
    let err = LaunchArgs::parse(["--nope"]).unwrap_err();
    assert_eq!(err.0, "--nope");
    let a = LaunchArgs::parse_lenient(["-ToastActivated", "--show", "junk"]);
    assert!(a.show && !a.background);
}

/// The panel is wanted for a plain launch and for `--show`, not for pure `--background`.
#[test]
fn test_cli_wants_panel_rules() {
    assert!(LaunchArgs::default().wants_panel());
    assert!(!LaunchArgs::parse(["--background"]).unwrap().wants_panel());
    assert!(
        LaunchArgs::parse(["--background", "--show"])
            .unwrap()
            .wants_panel()
    );
}

// ---- launch at login -----------------------------------------------------

/// Register writes the quoted command with `--background`, and status reads it back.
#[test]
fn test_autostart_register_writes_command_and_status_reads_back() {
    let reg = Arc::new(MemoryRegistry::new());
    let item = login(&reg, EXE);
    assert_eq!(item.status().unwrap(), LoginItemStatus::NotRegistered);
    item.register().unwrap();
    assert_eq!(
        reg.get_string(RUN_KEY, VALUE_NAME).unwrap().as_deref(),
        Some(r#""C:\Program Files\TokenMon\TokenMon.exe" --background"#)
    );
    assert_eq!(item.status().unwrap(), LoginItemStatus::Enabled);
}

/// Registering twice, and unregistering twice, never fails or duplicates.
#[test]
fn test_autostart_is_idempotent() {
    let reg = Arc::new(MemoryRegistry::new());
    let item = login(&reg, EXE);
    item.register().unwrap();
    item.register().unwrap();
    assert_eq!(item.status().unwrap(), LoginItemStatus::Enabled);
    item.unregister().unwrap();
    item.unregister().unwrap();
    assert_eq!(item.status().unwrap(), LoginItemStatus::NotRegistered);
    assert_eq!(reg.get_string(RUN_KEY, VALUE_NAME).unwrap(), None);
}

/// A Task Manager "disabled" blob (odd first byte) maps to `RequiresApproval`;
/// an "enabled" blob (even first byte) stays `Enabled`.
#[test]
fn test_autostart_requires_approval_when_disabled_in_task_manager() {
    let reg = Arc::new(MemoryRegistry::new());
    let item = login(&reg, EXE);
    item.register().unwrap();
    let mut blob = [0u8; 12];
    blob[0] = 0x03;
    reg.seed_binary(STARTUP_APPROVED_KEY, VALUE_NAME, &blob);
    assert_eq!(item.status().unwrap(), LoginItemStatus::RequiresApproval);
    blob[0] = 0x02;
    reg.seed_binary(STARTUP_APPROVED_KEY, VALUE_NAME, &blob);
    assert_eq!(item.status().unwrap(), LoginItemStatus::Enabled);
}

/// A disabled-looking blob without a Run entry is still `NotRegistered`, and
/// unregister clears the override so a later register starts clean.
#[test]
fn test_autostart_unregister_clears_override() {
    let reg = Arc::new(MemoryRegistry::new());
    let item = login(&reg, EXE);
    item.register().unwrap();
    reg.seed_binary(STARTUP_APPROVED_KEY, VALUE_NAME, &[3, 0, 0, 0]);
    item.unregister().unwrap();
    assert_eq!(item.status().unwrap(), LoginItemStatus::NotRegistered);
    item.register().unwrap();
    assert_eq!(item.status().unwrap(), LoginItemStatus::Enabled);
}

/// Re-registering after the exe moved rewrites the stale path.
#[test]
fn test_autostart_register_repairs_stale_path() {
    let reg = Arc::new(MemoryRegistry::new());
    login(&reg, r"C:\Old\TokenMon.exe").register().unwrap();
    let item = login(&reg, EXE);
    assert!(item.is_stale().unwrap());
    item.register().unwrap();
    assert!(!item.is_stale().unwrap());
    assert!(
        reg.get_string(RUN_KEY, VALUE_NAME)
            .unwrap()
            .unwrap()
            .contains("Program Files")
    );
}

/// Registry failures surface as errors and leave no entry behind.
#[test]
fn test_autostart_surfaces_registry_failure() {
    let reg = Arc::new(MemoryRegistry::new());
    reg.set_fail_writes(true);
    assert!(matches!(
        login(&reg, EXE).register(),
        Err(ShellError::Registry(_))
    ));
    assert_eq!(
        login(&reg, EXE).status().unwrap(),
        LoginItemStatus::NotRegistered
    );
}

/// A bad executable path is rejected before the registry is touched.
#[test]
fn test_autostart_rejects_bad_path_without_writing() {
    let reg = Arc::new(MemoryRegistry::new());
    assert!(login(&reg, "bad\"path").register().is_err());
    assert_eq!(reg.keys().len(), 0);
}

// ---- desktop-background verb --------------------------------------------

/// Install writes caption, icon and the `--show` command under the verb key.
#[test]
fn test_verb_install_layout() {
    let reg = Arc::new(MemoryRegistry::new());
    verb(&reg).install().unwrap();
    assert_eq!(
        reg.get_string(VERB_KEY, "").unwrap().as_deref(),
        Some("Show TokenMon")
    );
    assert_eq!(
        reg.get_string(VERB_KEY, "Icon").unwrap().as_deref(),
        Some(r#""C:\Program Files\TokenMon\TokenMon.exe",0"#)
    );
    assert_eq!(
        reg.get_string(COMMAND_KEY, "").unwrap().as_deref(),
        Some(r#""C:\Program Files\TokenMon\TokenMon.exe" --show"#)
    );
    assert!(verb(&reg).is_installed().unwrap());
}

/// The verb lives under HKCU\Software\Classes\DesktopBackground, nowhere else.
#[test]
fn test_verb_key_path_is_desktop_background_shell() {
    assert_eq!(
        VERB_KEY,
        r"Software\Classes\DesktopBackground\Shell\TokenMon"
    );
    assert_eq!(COMMAND_KEY, format!(r"{VERB_KEY}\command"));
}

/// Install is idempotent and uninstall removes every key it created, twice safely.
#[test]
fn test_verb_install_uninstall_idempotent_and_clean() {
    let reg = Arc::new(MemoryRegistry::new());
    let v = verb(&reg);
    v.install().unwrap();
    v.install().unwrap();
    v.uninstall().unwrap();
    v.uninstall().unwrap();
    assert!(!v.is_installed().unwrap());
    assert!(!reg.key_exists(VERB_KEY).unwrap());
    assert!(!reg.key_exists(COMMAND_KEY).unwrap());
}

/// Uninstall leaves sibling verbs (other apps) untouched.
#[test]
fn test_verb_uninstall_keeps_siblings() {
    let reg = Arc::new(MemoryRegistry::new());
    let other = r"Software\Classes\DesktopBackground\Shell\TokenMonOther";
    reg.set_string(other, "", "Other").unwrap();
    verb(&reg).install().unwrap();
    verb(&reg).uninstall().unwrap();
    assert_eq!(reg.get_string(other, "").unwrap().as_deref(), Some("Other"));
}

/// A bad path leaves nothing half-installed.
#[test]
fn test_verb_bad_path_writes_nothing() {
    let reg = Arc::new(MemoryRegistry::new());
    assert!(DesktopVerb::new(reg.clone(), "").install().is_err());
    assert_eq!(reg.keys().len(), 0);
}

// ---- single instance and startup ----------------------------------------

/// The second acquire sees the first; releasing the guard frees the lock.
#[test]
fn test_instance_lock_primary_then_secondary_then_free() {
    let lock = FakeInstanceLock::new();
    let first = lock.try_acquire().unwrap();
    assert!(matches!(first, InstanceState::Primary(_)));
    assert!(matches!(
        lock.clone().try_acquire().unwrap(),
        InstanceState::Secondary
    ));
    drop(first);
    assert!(matches!(
        lock.try_acquire().unwrap(),
        InstanceState::Primary(_)
    ));
}

/// First launch runs; it shows the panel unless it is a background launch.
#[test]
fn test_startup_primary_show_rules() {
    for (args, expect) in [
        (vec![], true),
        (vec!["--background"], false),
        (vec!["--show"], true),
        (vec!["--background", "--show"], true),
    ] {
        let lock = FakeInstanceLock::new();
        let d = decide_startup(LaunchArgs::parse(args).unwrap(), &lock, &FakeIpc::new()).unwrap();
        assert!(
            matches!(d, StartupDecision::RunPrimary { show_panel, .. } if show_panel == expect)
        );
    }
}

/// A second launch (or `--show`) asks the running instance to show its panel.
#[test]
fn test_startup_secondary_sends_show() {
    let lock = FakeInstanceLock::new();
    let ipc = FakeIpc::new();
    let _primary = decide_startup(LaunchArgs::default(), &lock, &ipc).unwrap();
    let d = decide_startup(LaunchArgs::parse(["--show"]).unwrap(), &lock, &ipc).unwrap();
    assert!(matches!(d, StartupDecision::HandedOff));
    assert_eq!(ipc.received(), vec![IpcMessage::Show]);
}

/// A second login-time `--background` launch exits silently without popping the panel.
#[test]
fn test_startup_secondary_background_sends_nothing() {
    let lock = FakeInstanceLock::new();
    let ipc = FakeIpc::new();
    let _primary = decide_startup(LaunchArgs::default(), &lock, &ipc).unwrap();
    let d = decide_startup(LaunchArgs::parse(["--background"]).unwrap(), &lock, &ipc).unwrap();
    assert!(matches!(d, StartupDecision::HandedOff));
    assert_eq!(ipc.received().len(), 0);
}

/// An unreachable primary is reported instead of starting a duplicate.
#[test]
fn test_startup_secondary_reports_unreachable_primary() {
    let lock = FakeInstanceLock::new();
    let _primary = decide_startup(LaunchArgs::default(), &lock, &FakeIpc::new()).unwrap();
    let d = decide_startup(LaunchArgs::default(), &lock, &FakeIpc::unreachable()).unwrap();
    assert!(matches!(
        d,
        StartupDecision::HandoffFailed(ShellError::Ipc(_))
    ));
}

// ---- IPC wire format -----------------------------------------------------

/// Encode then decode returns the same message; line endings are optional.
#[test]
fn test_ipc_roundtrip_and_line_endings() {
    assert_eq!(
        IpcMessage::decode(&IpcMessage::Show.encode()),
        Ok(IpcMessage::Show)
    );
    assert_eq!(IpcMessage::decode(b"TM1 show"), Ok(IpcMessage::Show));
    assert_eq!(IpcMessage::decode(b"TM1 show\r\n"), Ok(IpcMessage::Show));
}

/// Garbage, wrong case, non-UTF-8 and oversized input are typed errors.
#[test]
fn test_ipc_rejects_invalid_input() {
    assert_eq!(IpcMessage::decode(b""), Err(IpcParseError::Unknown));
    assert_eq!(IpcMessage::decode(b"show"), Err(IpcParseError::Unknown));
    assert_eq!(IpcMessage::decode(b"TM1 SHOW"), Err(IpcParseError::Unknown));
    assert_eq!(
        IpcMessage::decode(&[0xff, 0xfe]),
        Err(IpcParseError::Unknown)
    );
    assert_eq!(
        IpcMessage::decode([b'a'; MAX_MESSAGE_LEN + 1].as_slice()),
        Err(IpcParseError::TooLong)
    );
}

/// The reader never consumes more than the bound from a hostile stream.
#[test]
fn test_ipc_read_is_bounded() {
    let endless = std::io::repeat(b'x');
    assert_eq!(IpcMessage::read_from(endless), Err(IpcParseError::TooLong));
    assert_eq!(
        IpcMessage::read_from(&b"TM1 show\n"[..]),
        Ok(IpcMessage::Show)
    );
}

/// Pipe names are per user and contain only safe characters.
#[test]
fn test_pipe_name_sanitises_user() {
    assert_eq!(pipe_name("ada"), r"\\.\pipe\TokenMon.ada");
    assert_eq!(pipe_name(r"CORP\a/b:c"), r"\\.\pipe\TokenMon.CORP_a_b_c");
}
