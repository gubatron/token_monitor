//! TokenMon for Windows: the system-tray binary. Thin by design: everything
//! testable lives in the `tokenmon_app` library.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

fn init_logging() {
    // Ignore a second initialisation (tests, embedding).
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

fn main() -> ExitCode {
    init_logging();
    let args: Vec<String> = std::env::args().collect();
    let version = env!("CARGO_PKG_VERSION");
    if args.iter().any(|a| a == "--smoke") {
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
