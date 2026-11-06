//! TokenMon for Windows: the system-tray binary.
//!
//! Owner: ws-app (wiring, provider registry, polling orchestration, tray
//! event loop). This stub only initialises logging and exits.

#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    tracing::info!("TokenMon {} starting (stub)", env!("CARGO_PKG_VERSION"));
}
