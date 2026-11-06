//! TokenMon for Windows: the system-tray binary. Thin by design: everything
//! testable lives in the `tokenmon_app` library.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

/// Logs to stdout for the headless smoke run (CI reads it).
fn init_stdout_logging() {
    // Ignore a second initialisation (tests, embedding).
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

/// Logs to `%LOCALAPPDATA%\TokenMon\logs` (the app has no console) and
/// records panics before the release profile aborts. Falls back to stdout
/// when the folder is unavailable.
fn init_file_logging() {
    let dir = tokenmon_core::storage::AppPaths::production()
        .ok()
        .and_then(|p| p.directory().ok());
    let started = dir.is_some_and(|d| tokenmon_app::logging::init_file_logging(&d).is_ok());
    if !started {
        init_stdout_logging();
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let version = env!("CARGO_PKG_VERSION");
    let smoke = args.iter().any(|a| a == "--smoke");
    if smoke {
        init_stdout_logging();
    } else {
        init_file_logging();
    }
    if smoke {
        return if tokenmon_app::smoke::run_headless(version) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    run_app(&args, version)
}

#[cfg(windows)]
fn run_app(args: &[String], version: &str) -> ExitCode {
    tokenmon_app::shell::run(args, version)
}

#[cfg(not(windows))]
fn run_app(_args: &[String], version: &str) -> ExitCode {
    tracing::error!("TokenMon {version} is a Windows application; use --smoke to self-test");
    ExitCode::FAILURE
}
