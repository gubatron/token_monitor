//! Capped, hashed download of a release asset.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::redirect::Policy;
use tokenmon_core::BoxFuture;
use tokenmon_core::update::{InstallError, ReleaseSource, Sha256Stream, is_trusted_download};

/// A finished download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedFile {
    /// Where the bytes were written.
    pub path: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// Lowercase hex SHA-256 of the bytes.
    pub sha256: String,
}

/// Downloads one URL to a file, enforcing a size cap.
pub trait Downloader: Send + Sync {
    /// Streams `url` into `dest`, failing (and removing `dest`) when more than
    /// `max_bytes` would be written.
    fn download<'a>(
        &'a self,
        url: &'a str,
        dest: &'a Path,
        max_bytes: u64,
    ) -> BoxFuture<'a, Result<DownloadedFile, InstallError>>;
}

/// Which hosts a download (and its redirects) may touch.
#[derive(Debug, Clone)]
pub enum TrustPolicy {
    /// Only this repository's release downloads and GitHub's asset CDN hosts.
    Repository(ReleaseSource),
    /// No host restriction. Local mock servers in tests only.
    AnyHostForTests,
}

impl TrustPolicy {
    fn trusts(&self, url: &str) -> bool {
        match self {
            Self::Repository(s) => is_trusted_download(url, &s.owner, &s.repository),
            Self::AnyHostForTests => true,
        }
    }
}

/// `reqwest`-backed [`Downloader`].
#[derive(Debug, Clone)]
pub struct HttpDownloader {
    client: reqwest::Client,
    trust: TrustPolicy,
}

const MAX_REDIRECTS: usize = 5;

impl HttpDownloader {
    /// Builds a downloader. Redirects to untrusted URLs are not followed.
    ///
    /// # Errors
    ///
    /// [`InstallError::Download`] if the TLS backend cannot be initialised.
    pub fn new(trust: TrustPolicy, user_agent: &str) -> Result<Self, InstallError> {
        let policy_trust = trust.clone();
        let redirect = Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS
                || !policy_trust.trusts(attempt.url().as_str())
            {
                attempt.stop()
            } else {
                attempt.follow()
            }
        });
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .redirect(redirect)
            .build()
            .map_err(|e| InstallError::Download(e.to_string()))?;
        Ok(Self { client, trust })
    }

    async fn fetch(
        &self,
        url: &str,
        dest: &Path,
        max_bytes: u64,
    ) -> Result<DownloadedFile, InstallError> {
        let too_large = || InstallError::Download("The release file is unexpectedly large.".into());
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| InstallError::Download(e.without_url().to_string()))?;
        if !response.status().is_success() {
            return Err(InstallError::Download(
                "The release file was not available.".into(),
            ));
        }
        if !self.trust.trusts(response.url().as_str()) {
            return Err(InstallError::Download(
                "The release file is not hosted on GitHub.".into(),
            ));
        }
        if response.content_length().is_some_and(|n| n > max_bytes) {
            return Err(too_large());
        }
        let mut file = File::create(dest).map_err(|e| InstallError::Download(e.to_string()))?;
        let mut hasher = Sha256Stream::new();
        let mut size: u64 = 0;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| InstallError::Download(e.without_url().to_string()))?
        {
            size = size.saturating_add(chunk.len() as u64);
            if size > max_bytes {
                return Err(too_large());
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .map_err(|e| InstallError::Download(e.to_string()))?;
        }
        file.flush()
            .map_err(|e| InstallError::Download(e.to_string()))?;
        Ok(DownloadedFile {
            path: dest.to_path_buf(),
            size,
            sha256: hasher.finish_hex(),
        })
    }
}

impl Downloader for HttpDownloader {
    fn download<'a>(
        &'a self,
        url: &'a str,
        dest: &'a Path,
        max_bytes: u64,
    ) -> BoxFuture<'a, Result<DownloadedFile, InstallError>> {
        Box::pin(async move {
            let result = self.fetch(url, dest, max_bytes).await;
            if result.is_err() {
                let _ = std::fs::remove_file(dest);
            }
            result
        })
    }
}
