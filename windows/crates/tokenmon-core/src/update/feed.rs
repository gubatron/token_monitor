//! GitHub release feed parsing and Windows asset selection (spec 01 section 3.3).

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::error::UpdateError;
use super::trust::is_repository_release_download;
use super::version::AppVersion;
use crate::provider::BoxFuture;

/// Largest feed payload accepted: 2 MiB. A real `releases` page is far smaller.
pub const MAX_FEED_BYTES: usize = 2 * 1024 * 1024;

/// Most releases examined from a list payload.
const MAX_RELEASES_EXAMINED: usize = 100;

/// Where releases live and how the Windows ones are recognised.
///
/// # Examples
///
/// ```
/// use tokenmon_core::update::ReleaseSource;
///
/// let s = ReleaseSource::default();
/// assert!(s.releases_url().starts_with("https://api.github.com/repos/"));
/// assert_eq!(s.tag_prefix, "windows-v");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseSource {
    /// GitHub owner.
    pub owner: String,
    /// GitHub repository.
    pub repository: String,
    /// Required (lowercase) tag prefix; the version follows it.
    pub tag_prefix: String,
    /// Required (lowercase) asset-name prefix.
    pub asset_prefix: String,
}

impl Default for ReleaseSource {
    fn default() -> Self {
        Self {
            owner: "gubatron".to_owned(),
            repository: "token_monitor".to_owned(),
            tag_prefix: "windows-v".to_owned(),
            asset_prefix: "tokenmon-windows-".to_owned(),
        }
    }
}

impl ReleaseSource {
    /// The release-list endpoint. The repository also publishes other
    /// platforms' releases, so `releases/latest` could be a macOS release that
    /// would hide a newer Windows one; the list is scanned instead.
    pub fn releases_url(&self) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/releases?per_page=30",
            self.owner, self.repository
        )
    }

    /// The `releases/latest` endpoint (accepted by [`newer_release`] too).
    pub fn latest_url(&self) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            self.owner, self.repository
        )
    }

    /// The releases web page, used as the fallback "page" URL.
    pub fn releases_page_url(&self) -> String {
        format!(
            "https://github.com/{}/{}/releases/latest",
            self.owner, self.repository
        )
    }
}

/// Which kind of installable file an asset is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    /// Portable zip containing `TokenMon.exe`.
    Zip,
    /// Windows Installer package.
    Msi,
}

/// A file attached to a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    /// File name.
    pub name: String,
    /// `browser_download_url`; already checked as a repository download.
    pub url: String,
    /// Lowercase hex SHA-256 from GitHub's `digest`, when present and valid.
    pub sha256: Option<String>,
}

/// A published release newer than the running build.
#[derive(Debug, Clone)]
pub struct AvailableRelease {
    /// Parsed version.
    pub version: AppVersion,
    /// Release tag as published (`windows-v1.2.3`).
    pub tag: String,
    /// Release web page.
    pub page_url: String,
    /// Portable zip, when published.
    pub archive: Option<ReleaseAsset>,
    /// MSI, when published.
    pub installer: Option<ReleaseAsset>,
    /// Publication time, when reported.
    pub published_at: Option<DateTime<Utc>>,
}

impl PartialEq for AvailableRelease {
    /// Two releases are equal when version and page match.
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version && self.page_url == other.page_url
    }
}

/// Fetches the raw release feed. Implemented over HTTP in `tokenmon-platform`.
pub trait ReleaseFeedFetcher: Send + Sync {
    /// GETs the feed and returns the body, or the mapped error.
    fn fetch(&self) -> BoxFuture<'_, Result<Vec<u8>, UpdateError>>;
}

/// Lowercase hex SHA-256 from a GitHub asset digest (`sha256:<64 hex>`).
///
/// # Examples
///
/// ```
/// use tokenmon_core::update::sha256_from_digest;
///
/// let hex = "AB".repeat(32);
/// assert_eq!(sha256_from_digest(&format!("SHA256:{hex}")), Some("ab".repeat(32)));
/// assert_eq!(sha256_from_digest("md5:abc"), None);
/// ```
pub fn sha256_from_digest(digest: &str) -> Option<String> {
    let (algo, hex) = digest.split_once(':')?;
    (algo.eq_ignore_ascii_case("sha256")
        && hex.len() == 64
        && hex.bytes().all(|b| b.is_ascii_hexdigit()))
    .then(|| hex.to_ascii_lowercase())
}

fn first_string<'a>(obj: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| obj.get(*k)?.as_str())
}

fn is_true(obj: &Value, key: &str) -> bool {
    obj.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Decides whether the feed offers a Windows release strictly newer than
/// `current`.
///
/// Accepts either a single release object (`releases/latest`) or an array
/// (`releases`); from an array the newest eligible release wins and malformed
/// entries are skipped. Drafts, prereleases and releases whose tag does not
/// start with the source's `tag_prefix` are ignored.
///
/// # Errors
///
/// [`UpdateError::BadResponse`] when the payload is oversized or not JSON
/// objects, or a single Windows release carries an unparseable tag.
///
/// # Examples
///
/// ```
/// use tokenmon_core::update::{AppVersion, ReleaseSource, newer_release};
///
/// let body = br#"{"tag_name":"windows-v1.2.0","html_url":"https://github.com/gubatron/token_monitor/releases/tag/windows-v1.2.0"}"#;
/// let current = AppVersion::parse("1.1.0").unwrap();
/// let r = newer_release(body, &current, &ReleaseSource::default()).unwrap().unwrap();
/// assert_eq!(r.version.to_string(), "1.2.0");
/// ```
pub fn newer_release(
    data: &[u8],
    current: &AppVersion,
    source: &ReleaseSource,
) -> Result<Option<AvailableRelease>, UpdateError> {
    if data.len() > MAX_FEED_BYTES {
        return Err(UpdateError::BadResponse(
            "Release payload is too large".into(),
        ));
    }
    let root: Value = serde_json::from_slice(data)
        .map_err(|_| UpdateError::BadResponse("Unexpected release payload".into()))?;
    match &root {
        Value::Object(_) => evaluate(&root, current, source),
        Value::Array(items) => Ok(items
            .iter()
            .take(MAX_RELEASES_EXAMINED)
            .filter_map(|item| evaluate(item, current, source).ok().flatten())
            .max_by(|a, b| a.version.cmp(&b.version))),
        _ => Err(UpdateError::BadResponse(
            "Unexpected release payload".into(),
        )),
    }
}

fn evaluate(
    root: &Value,
    current: &AppVersion,
    source: &ReleaseSource,
) -> Result<Option<AvailableRelease>, UpdateError> {
    if !root.is_object() {
        return Err(UpdateError::BadResponse(
            "Unexpected release payload".into(),
        ));
    }
    if is_true(root, "draft") || is_true(root, "prerelease") {
        return Ok(None);
    }
    let tag = first_string(root, &["tag_name", "name"])
        .ok_or_else(|| UpdateError::BadResponse("Release carries no usable version tag".into()))?;
    let Some(version_text) = strip_tag_prefix(tag, &source.tag_prefix) else {
        return Ok(None);
    };
    let version = AppVersion::parse(version_text)
        .ok_or_else(|| UpdateError::BadResponse("Release carries no usable version tag".into()))?;
    if version <= *current || version.is_prerelease() {
        return Ok(None);
    }
    let assets = release_assets(root, source);
    Ok(Some(AvailableRelease {
        version,
        tag: tag.to_owned(),
        page_url: first_string(root, &["html_url"])
            .map_or_else(|| source.releases_page_url(), str::to_owned),
        archive: preferred_asset(&assets, "zip", source),
        installer: preferred_asset(&assets, "msi", source),
        published_at: first_string(root, &["published_at"])
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc)),
    }))
}

fn strip_tag_prefix<'a>(tag: &'a str, prefix: &str) -> Option<&'a str> {
    let head = tag.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &tag[prefix.len()..])
}

/// Assets whose download URL is one of this repository's release downloads;
/// anything else is dropped whatever the payload lists.
fn release_assets(root: &Value, source: &ReleaseSource) -> Vec<ReleaseAsset> {
    let Some(items) = root.get("assets").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|a| {
            let name = first_string(a, &["name"])?;
            let url = first_string(a, &["browser_download_url"])?;
            if name.contains(['/', '\\'])
                || !is_repository_release_download(url, &source.owner, &source.repository)
            {
                return None;
            }
            Some(ReleaseAsset {
                name: name.to_owned(),
                url: url.to_owned(),
                sha256: first_string(a, &["digest"]).and_then(sha256_from_digest),
            })
        })
        .collect()
}

/// First asset named `<asset_prefix>*.<ext>`, skipping symbol bundles. Names
/// without the Windows prefix (the macOS zip, for one) are never selected.
fn preferred_asset(
    assets: &[ReleaseAsset],
    ext: &str,
    source: &ReleaseSource,
) -> Option<ReleaseAsset> {
    let suffix = format!(".{ext}");
    assets
        .iter()
        .find(|a| {
            let n = a.name.to_ascii_lowercase();
            n.starts_with(&source.asset_prefix)
                && n.ends_with(&suffix)
                && !n.contains("dsym")
                && !n.contains("pdb")
                && !n.contains("symbols")
        })
        .cloned()
}
