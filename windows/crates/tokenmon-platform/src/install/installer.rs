//! The orchestrating [`UpdateInstaller`] for Windows.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::{Builder, TempDir};
use tokenmon_core::BoxFuture;
use tokenmon_core::update::{
    AppVersion, AssetKind, InstallError, InstallOutcome, InstallRequest, MAX_DOWNLOAD_BYTES,
    MAX_EXTRACTED_BYTES, ReleaseSource, Sha256Stream, UpdateInstaller,
    is_repository_release_download,
};

use crate::shell::Registry;

use super::archive::{extract_zip, locate_exe};
use super::download::Downloader;
use super::pe_version::{PackageVersionReader, SystemVersionReader};
use super::relaunch::{ProcessLauncher, msiexec_spec, relaunch_spec};
use super::swap::{can_replace_running_exe, commit_swap, stage_next_to};
use super::verify::SignatureVerifier;

/// Installs an update over the running executable (zip) or via `msiexec` (MSI).
///
/// All side effects go through injected collaborators, so the whole flow runs
/// against temp directories and fakes in tests.
pub struct WindowsUpdateInstaller {
    source: ReleaseSource,
    running_exe: PathBuf,
    process_id: u32,
    work_root: PathBuf,
    downloader: Arc<dyn Downloader>,
    verifier: Arc<dyn SignatureVerifier>,
    launcher: Arc<dyn ProcessLauncher>,
    versions: Arc<dyn PackageVersionReader>,
    registry: Option<Arc<dyn Registry>>,
}

impl WindowsUpdateInstaller {
    /// Wires the installer.
    ///
    /// `work_root` is where per-update staging directories are created
    /// (normally `std::env::temp_dir()`); `process_id` is the running app's pid.
    pub fn new(
        source: ReleaseSource,
        running_exe: PathBuf,
        process_id: u32,
        work_root: PathBuf,
        downloader: Arc<dyn Downloader>,
        verifier: Arc<dyn SignatureVerifier>,
        launcher: Arc<dyn ProcessLauncher>,
    ) -> Self {
        Self {
            source,
            running_exe,
            process_id,
            work_root,
            downloader,
            verifier,
            launcher,
            versions: Arc::new(SystemVersionReader),
            registry: None,
        }
    }

    /// Replaces how a downloaded binary's own version is read (tests).
    #[must_use]
    pub fn with_version_reader(mut self, versions: Arc<dyn PackageVersionReader>) -> Self {
        self.versions = versions;
        self
    }

    /// Supplies the registry the MSI's `InstallDir` record is read from. Without
    /// it the app is never treated as MSI-installed.
    #[must_use]
    pub fn with_registry(mut self, registry: Arc<dyn Registry>) -> Self {
        self.registry = Some(registry);
        self
    }

    fn work_dir(&self) -> Result<TempDir, InstallError> {
        Builder::new()
            .prefix("tokenmon-update-")
            .tempdir_in(&self.work_root)
            .map_err(|e| InstallError::Install(e.to_string()))
    }

    /// Downloads and checks the digest; the file lands in `dir`.
    async fn fetch_verified(
        &self,
        req: &InstallRequest,
        dir: &Path,
        file_name: &str,
    ) -> Result<PathBuf, InstallError> {
        let dest = dir.join(file_name);
        let got = self
            .downloader
            .download(&req.asset.url, &dest, MAX_DOWNLOAD_BYTES)
            .await?;
        if got.sha256 != req.expected_sha256.to_ascii_lowercase() {
            let _ = std::fs::remove_file(&dest);
            return Err(InstallError::Verification(
                "The download does not match the release checksum.".into(),
            ));
        }
        Ok(got.path)
    }

    fn precheck(&self, req: &InstallRequest) -> Result<(), InstallError> {
        if !is_repository_release_download(
            &req.asset.url,
            &self.source.owner,
            &self.source.repository,
        ) {
            return Err(InstallError::Download(
                "The release file is not one of this project's GitHub releases.".into(),
            ));
        }
        if req.target <= req.current {
            return Err(InstallError::Verification(
                "The download is not newer than this version.".into(),
            ));
        }
        if req.kind == AssetKind::Zip && !can_replace_running_exe(&self.running_exe) {
            return Err(InstallError::NotWritable);
        }
        Ok(())
    }

    /// The downloaded binary's own version must match the release and be newer
    /// than the running app: the tag alone could front a replayed older build (#77).
    fn check_declared_version(
        req: &InstallRequest,
        declared: Result<AppVersion, InstallError>,
    ) -> Result<(), InstallError> {
        let declared = declared?;
        if declared <= req.current {
            return Err(InstallError::Verification(
                "The download is not newer than this version.".into(),
            ));
        }
        if declared != req.target {
            return Err(InstallError::Verification(
                "The download is not the version the release announced.".into(),
            ));
        }
        Ok(())
    }

    async fn install_zip(&self, req: &InstallRequest) -> Result<InstallOutcome, InstallError> {
        let work = self.work_dir()?;
        let zip = self
            .fetch_verified(req, work.path(), "TokenMon.zip")
            .await?;
        let unpacked = work.path().join("unpacked");
        extract_zip(&zip, &unpacked, MAX_EXTRACTED_BYTES)?;
        let new_exe = locate_exe(&unpacked)?;
        self.verifier.verify_exe(&new_exe)?;
        Self::check_declared_version(req, self.versions.exe_version(&new_exe))?;
        let staged = stage_next_to(&new_exe, &self.running_exe)?;
        self.verifier.verify_exe(staged.path())?;
        // What gets installed is the staged copy, so check that one too.
        Self::check_declared_version(req, self.versions.exe_version(staged.path()))?;
        commit_swap(staged, &self.running_exe)?;
        Ok(self.relaunch())
    }

    fn relaunch(&self) -> InstallOutcome {
        let spec = relaunch_spec(&self.running_exe, self.process_id);
        match self.launcher.spawn_detached(&spec) {
            Ok(()) => InstallOutcome::Relaunching,
            Err(e) => {
                tracing::warn!(error = %e, "relaunch failed");
                InstallOutcome::InstalledNeedsManualRestart
            }
        }
    }

    async fn install_msi(&self, req: &InstallRequest) -> Result<InstallOutcome, InstallError> {
        // The package must outlive this call (msiexec runs after the app quits),
        // so its folder is deliberately kept.
        let dir = self.work_dir()?.keep();
        let result = self.run_msi(req, &dir).await;
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        result
    }

    async fn run_msi(
        &self,
        req: &InstallRequest,
        dir: &Path,
    ) -> Result<InstallOutcome, InstallError> {
        let msi = self.fetch_verified(req, dir, "TokenMon.msi").await?;
        self.verifier.verify_msi(&msi)?;
        Self::check_declared_version(req, self.versions.msi_version(&msi))?;
        // Close the verify-to-run gap: the file must still be the checked
        // download at the moment msiexec is spawned.
        if hash_file(&msi)? != req.expected_sha256.to_ascii_lowercase() {
            return Err(InstallError::Verification(
                "The package changed after it was verified.".into(),
            ));
        }
        self.launcher
            .spawn_detached(&msiexec_spec(&msi))
            .map_err(|e| InstallError::Install(e.to_string()))?;
        Ok(InstallOutcome::Relaunching)
    }
}

/// Lowercase hex SHA-256 of a file, streamed.
fn hash_file(path: &Path) -> Result<String, InstallError> {
    use std::io::Read;
    let io = |e: std::io::Error| InstallError::Install(e.to_string());
    let mut file = std::fs::File::open(path).map_err(io)?;
    let mut hasher = Sha256Stream::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(io)?;
        if n == 0 {
            return Ok(hasher.finish_hex());
        }
        hasher.update(&buf[..n]);
    }
}

/// Key the MSI writes under `HKCU` (see `installer/main.wxs`).
pub const MSI_INSTALLER_KEY: &str = r"Software\TokenMon\Installer";
/// Value under [`MSI_INSTALLER_KEY`] holding the MSI's install folder.
pub const MSI_INSTALL_DIR_VALUE: &str = "InstallDir";

/// Lowercased path with `/` as `\` and no trailing separator, so the registry's
/// `C:\...\TokenMon\` and the process's `C:\...\TokenMon` compare equal.
fn normalized_dir(path: &str) -> String {
    path.replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// Whether `running_exe` lives directly in the folder the MSI recorded.
///
/// A portable copy elsewhere on disk, or a different folder, is not MSI-owned
/// even when the MSI is also installed.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use tokenmon_platform::install::installer::exe_in_msi_install_dir;
///
/// let exe = Path::new(r"C:\Users\a\AppData\Local\Programs\TokenMon\TokenMon.exe");
/// assert!(exe_in_msi_install_dir(exe, r"C:\Users\a\AppData\Local\Programs\TokenMon\"));
/// assert!(!exe_in_msi_install_dir(exe, r"D:\Portable"));
/// ```
#[must_use]
pub fn exe_in_msi_install_dir(running_exe: &Path, install_dir: &str) -> bool {
    let recorded = normalized_dir(install_dir);
    if recorded.is_empty() {
        return false;
    }
    // String-based so a Windows-style path splits the same way on any host.
    running_exe.to_str().is_some_and(|exe| {
        let exe = exe.replace('/', "\\");
        exe.rsplit_once('\\')
            .is_some_and(|(parent, _)| normalized_dir(parent) == recorded)
    })
}

impl UpdateInstaller for WindowsUpdateInstaller {
    fn installed_by_msi(&self) -> bool {
        let Some(registry) = &self.registry else {
            return false;
        };
        matches!(
            registry.get_string(MSI_INSTALLER_KEY, MSI_INSTALL_DIR_VALUE),
            Ok(Some(dir)) if exe_in_msi_install_dir(&self.running_exe, &dir)
        )
    }

    fn can_replace_running_app(&self) -> bool {
        can_replace_running_exe(&self.running_exe)
    }

    fn install(
        &self,
        request: InstallRequest,
    ) -> BoxFuture<'_, Result<InstallOutcome, InstallError>> {
        Box::pin(async move {
            self.precheck(&request)?;
            match request.kind {
                AssetKind::Zip => self.install_zip(&request).await,
                AssetKind::Msi => self.install_msi(&request).await,
            }
        })
    }
}
