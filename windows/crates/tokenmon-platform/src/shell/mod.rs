//! Autostart Run key, Explorer context-menu verb, single instance and the "show" IPC.
//!
//! Owner: ws-shell.
//!
//! Everything that touches the OS sits behind a trait ([`Registry`],
//! [`InstanceLock`], [`IpcClient`]) with an in-memory fake, so the logic
//! (command-line quoting, key layout, idempotence, startup decisions, wire
//! format) is tested on macOS and Linux. The `cfg(windows)` implementations
//! live in `windows.rs`.
//!
//! * [`autostart`]: launch at login via `HKCU\...\Run` ([`LoginItemService`]).
//! * [`context_menu`]: opt-in desktop-background "Show TokenMon" verb.
//! * [`instance`] and [`ipc`]: single instance plus the "show the panel" message.
//! * [`startup`]: combines the above into the decision `main` makes at launch.
//! * [`cli`]: `--background` / `--show` argument parsing.

pub mod autostart;
pub mod cli;
pub mod command_line;
pub mod context_menu;
pub mod error;
pub mod instance;
pub mod ipc;
pub mod registry;
pub mod startup;
#[cfg(windows)]
pub mod windows;

pub use autostart::{LoginItemService, LoginItemStatus, RunKeyLoginItem};
pub use cli::{LaunchArgs, LaunchArgsError};
pub use context_menu::{DesktopVerb, ShellVerbService};
pub use error::ShellError;
pub use instance::{FakeInstanceLock, InstanceGuard, InstanceLock, InstanceState};
pub use ipc::{FakeIpc, IpcClient, IpcMessage, IpcParseError};
pub use registry::{MemoryRegistry, Registry};
pub use startup::{StartupDecision, decide_startup};
