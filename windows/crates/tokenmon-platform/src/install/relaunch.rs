//! Process launching for relaunch and MSI installs.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// A program plus arguments, decoupled from `std::process` so it is testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// Executable to run.
    pub program: OsString,
    /// Arguments, passed without shell interpretation.
    pub args: Vec<OsString>,
}

/// Launches a detached process.
pub trait ProcessLauncher: Send + Sync {
    /// Starts `spec` without waiting for it.
    fn spawn_detached(&self, spec: &CommandSpec) -> io::Result<()>;
}

/// Argument the relaunched build receives so it waits for the old process to
/// exit (single-instance guard) before it takes over.
pub const RELAUNCH_WAIT_ARG: &str = "--wait-for-pid";

/// Command that starts the freshly installed `exe` once process `pid` is gone.
///
/// The app (ws-app) must honour [`RELAUNCH_WAIT_ARG`]: wait for `pid` to exit
/// (bounded), then continue startup normally.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use tokenmon_platform::install::relaunch::relaunch_spec;
///
/// let spec = relaunch_spec(Path::new(r"C:\Apps\TokenMon.exe"), 42);
/// assert_eq!(spec.args, ["--wait-for-pid", "42"]);
/// ```
pub fn relaunch_spec(exe: &Path, pid: u32) -> CommandSpec {
    CommandSpec {
        program: exe.as_os_str().to_owned(),
        args: vec![RELAUNCH_WAIT_ARG.into(), pid.to_string().into()],
    }
}

/// The Windows system directory (`C:\Windows\System32`), as an absolute path.
///
/// Asked from the OS (`GetSystemDirectoryW`); if that fails, `%SystemRoot%`
/// and finally `C:\Windows` are used. Never a relative path, so a program
/// resolved under it cannot be shadowed by the application directory or the
/// current directory.
pub fn system_directory() -> String {
    #[cfg(windows)]
    if let Some(dir) = os_system_directory() {
        return dir;
    }
    let root = std::env::var("SystemRoot")
        .ok()
        .filter(|r| is_absolute_windows_path(r))
        .unwrap_or_else(|| r"C:\Windows".to_owned());
    format!("{}\\System32", root.trim_end_matches(['\\', '/']))
}

#[cfg(windows)]
fn os_system_directory() -> Option<String> {
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buf = [0u16; 260];
    // SAFETY: `buf` is valid for `buf.len()` u16 writes.
    let len = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if len == 0 || len >= buf.len() {
        return None;
    }
    let dir = String::from_utf16(&buf[..len]).ok()?;
    is_absolute_windows_path(&dir).then_some(dir)
}

/// `X:\...` or `\\server\...`.
fn is_absolute_windows_path(p: &str) -> bool {
    let b = p.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        || p.starts_with("\\\\")
}

/// Per-user, unattended-with-progress `msiexec` run of a downloaded package.
///
/// `msiexec.exe` is addressed by its absolute path under the system directory
/// ([`system_directory`]); a bare name would be looked up in the application's
/// own (user-writable) folder first.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use tokenmon_platform::install::relaunch::msiexec_spec;
///
/// let spec = msiexec_spec(Path::new(r"C:\t\x.msi"));
/// let program = spec.program.to_string_lossy().to_lowercase();
/// assert!(program.ends_with(r"\system32\msiexec.exe"), "{program}");
/// assert!(spec.args.iter().any(|a| a == "MSIINSTALLPERUSER=1"));
/// ```
pub fn msiexec_spec(msi: &Path) -> CommandSpec {
    msiexec_spec_in(&system_directory(), msi)
}

/// [`msiexec_spec`] with the system directory supplied (testable).
pub fn msiexec_spec_in(system_dir: &str, msi: &Path) -> CommandSpec {
    CommandSpec {
        program: format!("{}\\msiexec.exe", system_dir.trim_end_matches(['\\', '/'])).into(),
        args: vec![
            "/i".into(),
            msi.as_os_str().to_owned(),
            "/passive".into(),
            "/norestart".into(),
            "ALLUSERS=2".into(),
            "MSIINSTALLPERUSER=1".into(),
        ],
    }
}

/// Real launcher. On Windows the child is created detached with no console
/// window so it outlives the quitting app.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdLauncher;

impl ProcessLauncher for StdLauncher {
    fn spawn_detached(&self, spec: &CommandSpec) -> io::Result<()> {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        }
        cmd.spawn().map(drop)
    }
}

/// Extracts the pid from a `--wait-for-pid <pid>` (or `--wait-for-pid=<pid>`)
/// command line. Returns `None` when absent or not a valid non-zero pid.
///
/// # Examples
///
/// ```
/// use tokenmon_platform::install::relaunch::parse_wait_for_pid;
///
/// assert_eq!(parse_wait_for_pid(["TokenMon.exe", "--wait-for-pid", "42"]), Some(42));
/// assert_eq!(parse_wait_for_pid(["x", "--wait-for-pid=7"]), Some(7));
/// assert_eq!(parse_wait_for_pid(["x", "--wait-for-pid", "abc"]), None);
/// assert_eq!(parse_wait_for_pid(["x", "--wait-for-pid", "0"]), None);
/// assert_eq!(parse_wait_for_pid(["x"]), None);
/// ```
pub fn parse_wait_for_pid<I, S>(args: I) -> Option<u32>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let arg = arg.as_ref();
        let value = if arg == RELAUNCH_WAIT_ARG {
            it.next().map(|v| v.as_ref().to_owned())
        } else {
            arg.strip_prefix("--wait-for-pid=").map(str::to_owned)
        };
        if let Some(v) = value {
            return v.trim().parse::<u32>().ok().filter(|p| *p != 0);
        }
    }
    None
}

/// Longest the new process waits for the old one to exit.
pub const WAIT_FOR_PID_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Blocks until process `pid` has exited or `timeout` elapses; returns `true`
/// when it is gone (or cannot be opened, which means it is already gone).
/// On non-Windows hosts it returns `true` immediately.
pub fn wait_for_pid_exit(pid: u32, timeout: std::time::Duration) -> bool {
    imp::wait(pid, timeout)
}

/// Startup helper for the app (ws-app): call once, first thing in `main`,
/// before the single-instance guard.
///
/// 1. If the command line has `--wait-for-pid <pid>` (the previous build is
///    exiting after an update), waits for it, bounded by
///    [`WAIT_FOR_PID_TIMEOUT`].
/// 2. Calls [`super::swap::cleanup_leftovers`] for `running_exe` to delete the
///    renamed-aside old exe and abandoned staged copies.
///
/// Returns the pid that was waited for, if any.
pub fn handle_startup_args<I, S>(args: I, running_exe: &Path) -> Option<u32>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let waited = parse_wait_for_pid(args);
    if let Some(pid) = waited {
        wait_for_pid_exit(pid, WAIT_FOR_PID_TIMEOUT);
    }
    super::swap::cleanup_leftovers(running_exe);
    waited
}

#[cfg(windows)]
mod imp {
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    pub fn wait(pid: u32, timeout: Duration) -> bool {
        // SAFETY: plain Win32 calls; the handle is closed before returning.
        unsafe {
            let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if handle.is_null() {
                return true;
            }
            let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
            let r = WaitForSingleObject(handle, ms);
            CloseHandle(handle);
            r != WAIT_TIMEOUT
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::time::Duration;

    pub fn wait(_pid: u32, _timeout: Duration) -> bool {
        true
    }
}
