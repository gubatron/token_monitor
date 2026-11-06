//! Update download, verification, staged replace and relaunch (spec 01 section 3.5).
//!
//! Nothing here is `cfg(windows)`-only: the logic works on any OS against temp
//! directories (the "running exe" is just a file), so it is tested on macOS and
//! Linux. Only [`relaunch::StdLauncher`] adds Windows process-creation flags.
//!
//! Flow for the portable zip: download to a staging directory (80 MiB cap,
//! trusted redirects only) -> SHA-256 must equal the GitHub digest -> extract
//! with size/symlink/path limits -> require a top-level `TokenMon.exe` ->
//! [`verify`] hook -> stage a copy beside the running exe -> rename the running
//! exe aside, rename the staged copy into place -> relaunch. Any failure leaves
//! the installed exe untouched. MSI releases are downloaded, verified and
//! handed to `msiexec` instead.

pub mod archive;
pub mod download;
pub mod feed;
pub mod installer;
pub mod pe_version;
pub mod relaunch;
pub mod swap;
pub mod verify;

pub use download::{DownloadedFile, Downloader, HttpDownloader, TrustPolicy};
pub use feed::HttpFeedFetcher;
pub use installer::WindowsUpdateInstaller;
pub use pe_version::{PackageVersionReader, SystemVersionReader, read_pe_version};
pub use relaunch::{
    CommandSpec, ProcessLauncher, StdLauncher, handle_startup_args, parse_wait_for_pid,
    wait_for_pid_exit,
};
pub use verify::{
    AuthenticodeVerifier, PeHeaderVerifier, SignaturePolicy, SignatureVerifier, verifier_for_policy,
};

/// File name of the application executable inside the release zip.
pub const EXE_NAME: &str = "TokenMon.exe";
