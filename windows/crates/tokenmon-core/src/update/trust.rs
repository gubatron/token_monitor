//! URL trust rules for release downloads (spec 01 section 3.3).
//!
//! The `url` crate silently normalizes `..` and `.` path segments away, which
//! would make `/releases/download/../x` look legitimate. These checks therefore
//! work on the **raw string** and reject dot segments (including percent-encoded
//! ones) before any normalization could happen.

/// Hosts GitHub redirects release-asset downloads to.
pub const ASSET_CDN_HOSTS: [&str; 2] = [
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// The pieces of a plain-HTTPS URL, borrowed from the raw input.
struct RawUrl<'a> {
    host: &'a str,
    /// Path segments after the leading `/`, exactly as written.
    segments: Vec<&'a str>,
    /// Whether a `?query` or `#fragment` was present.
    has_query_or_fragment: bool,
}

/// Splits `https://host[:443]/a/b` without normalizing anything.
///
/// Returns `None` for other schemes, embedded credentials, ports other than
/// 443, backslashes, whitespace/control characters or dot segments.
fn parse_plain_https(url: &str) -> Option<RawUrl<'_>> {
    let scheme_len = "https://".len();
    let head = url.get(..scheme_len)?;
    if !head.eq_ignore_ascii_case("https://") {
        return None;
    }
    if url
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return None;
    }
    let rest = &url[scheme_len..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    if authority.contains('@') {
        return None;
    }
    let host = match authority.split_once(':') {
        None => authority,
        Some((h, "443")) => h,
        Some(_) => return None,
    };
    if host.is_empty() {
        return None;
    }
    let (path, has_query_or_fragment) = match tail.find(['?', '#']) {
        Some(i) => (&tail[..i], true),
        None => (tail, false),
    };
    let path = path.strip_prefix('/').unwrap_or("");
    let segments: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    };
    if segments.iter().any(|s| is_dot_segment(s)) {
        return None;
    }
    Some(RawUrl {
        host,
        segments,
        has_query_or_fragment,
    })
}

/// `.`/`..`, including percent-encoded spellings such as `%2e%2E`.
fn is_dot_segment(segment: &str) -> bool {
    let lowered = segment.to_ascii_lowercase().replace("%2e", ".");
    lowered == "." || lowered == ".."
}

/// True for `https://github.com/<owner>/<repo>/releases/download/<tag>/<file>`.
///
/// Owner and repo compare case-insensitively; `releases` and `download` are
/// case-sensitive. The path must have exactly those six segments, none empty
/// or a dot segment, and there must be no query, fragment, credentials, or
/// port other than 443.
///
/// # Examples
///
/// ```
/// use tokenmon_core::update::is_repository_release_download;
///
/// let ok = "https://github.com/gubatron/token_monitor/releases/download/windows-v1.0.0/tokenmon-windows-x64.zip";
/// assert!(is_repository_release_download(ok, "gubatron", "token_monitor"));
/// let sneaky = "https://github.com/gubatron/token_monitor/releases/download/../../x/y/z";
/// assert!(!is_repository_release_download(sneaky, "gubatron", "token_monitor"));
/// ```
pub fn is_repository_release_download(url: &str, owner: &str, repository: &str) -> bool {
    let Some(raw) = parse_plain_https(url) else {
        return false;
    };
    if raw.has_query_or_fragment || !raw.host.eq_ignore_ascii_case("github.com") {
        return false;
    }
    match raw.segments.as_slice() {
        [o, r, "releases", "download", tag, file] => {
            o.eq_ignore_ascii_case(owner)
                && r.eq_ignore_ascii_case(repository)
                && !tag.is_empty()
                && !file.is_empty()
        }
        _ => false,
    }
}

/// True for this repository's release downloads and the GitHub CDN hosts those
/// downloads redirect to (query strings are allowed on the CDN, as signed
/// URLs need them).
///
/// # Examples
///
/// ```
/// use tokenmon_core::update::is_trusted_download;
///
/// assert!(is_trusted_download(
///     "https://objects.githubusercontent.com/github-production/abc?X-Amz=1",
///     "gubatron",
///     "token_monitor",
/// ));
/// assert!(!is_trusted_download("http://objects.githubusercontent.com/a", "gubatron", "token_monitor"));
/// ```
pub fn is_trusted_download(url: &str, owner: &str, repository: &str) -> bool {
    if is_repository_release_download(url, owner, repository) {
        return true;
    }
    parse_plain_https(url).is_some_and(|raw| {
        ASSET_CDN_HOSTS
            .iter()
            .any(|h| raw.host.eq_ignore_ascii_case(h))
    })
}
