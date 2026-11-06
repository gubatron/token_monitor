//! The orchestrating [`UpdateInstaller`] for Windows.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::{Builder, TempDir};
use tokenmon_core::BoxFuture;
use tokenmon_core::update::{
    AssetKind, InstallError, InstallOutcome, InstallRequest, MAX_DOWNLOAD_BYTES,
    MAX_EXTRACTED_BYTES, ReleaseSource, UpdateInstaller, is_repository_release_download,
};

use super::archive::{extract_zip, locate_exe};
use super::download::Downloader;
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
        }
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

    async fn install_zip(&self, req: &InstallRequest) -> Result<InstallOutcome, InstallError> {
        let work = self.work_dir()?;
        let zip = self
            .fetch_verified(req, work.path(), "TokenMon.zip")
            .await?;
        let unpacked = work.path().join("unpacked");
        extract_zip(&zip, &unpacked, MAX_EXTRACTED_BYTES)?;
        let new_exe = locate_exe(&unpacked)?;
        self.verifier.verify_exe(&new_exe)?;
        let staged = stage_next_to(&new_exe, &self.running_exe)?;
        self.verifier.verify_exe(staged.path())?;
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
        self.launcher
            .spawn_detached(&msiexec_spec(&msi))
            .map_err(|e| InstallError::Install(e.to_string()))?;
        Ok(InstallOutcome::Relaunching)
    }
}

impl UpdateInstaller for WindowsUpdateInstaller {
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
