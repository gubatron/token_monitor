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

/// Per-user, unattended-with-progress `msiexec` run of a downloaded package.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use tokenmon_platform::install::relaunch::msiexec_spec;
///
/// let spec = msiexec_spec(Path::new(r"C:\t\x.msi"));
/// assert_eq!(spec.program, "msiexec.exe");
/// assert!(spec.args.iter().any(|a| a == "MSIINSTALLPERUSER=1"));
/// ```
pub fn msiexec_spec(msi: &Path) -> CommandSpec {
    CommandSpec {
        program: "msiexec.exe".into(),
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
