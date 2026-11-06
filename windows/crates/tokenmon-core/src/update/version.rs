//! `AppVersion`: lenient parsing and semver-style ordering (spec 01 section 3.2).

use std::cmp::Ordering;
use std::fmt;

/// A parsed application version such as `1.8.0` or `1.8.0-beta.2`.
///
/// Equality follows ordering (`1.4 == 1.4.0`, build metadata is ignored), so
/// this type deliberately does not implement `Hash`.
///
/// # Examples
///
/// ```
/// use tokenmon_core::update::AppVersion;
///
/// let v = AppVersion::parse("TokenMon-v1.8.0-beta.2").unwrap();
/// assert_eq!(v.to_string(), "1.8.0-beta.2");
/// assert!(v.is_prerelease());
/// assert!(AppVersion::parse("1.10").unwrap() > AppVersion::parse("1.9.9").unwrap());
/// assert_eq!(AppVersion::parse("1.4"), AppVersion::parse("1.4.0"));
/// assert!(AppVersion::parse("nightly").is_none());
/// ```
#[derive(Debug, Clone)]
pub struct AppVersion {
    components: Vec<u64>,
    prerelease: Option<String>,
}

impl AppVersion {
    /// Parses a version string; `None` when it carries no usable version.
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        let no_meta = trimmed.split('+').next().unwrap_or("");
        let text = strip_release_name(no_meta);
        let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
        if text.is_empty() {
            return None;
        }
        let (core, suffix) = match text.split_once('-') {
            Some((c, s)) => (c, Some(s)),
            None => (text, None),
        };
        let components = core
            .split('.')
            .map(|p| p.parse::<u64>().ok())
            .collect::<Option<Vec<_>>>()?;
        let prerelease = suffix.filter(|s| !s.is_empty()).map(str::to_owned);
        Some(Self {
            components,
            prerelease,
        })
    }

    /// Whether the version carries a prerelease suffix.
    pub fn is_prerelease(&self) -> bool {
        self.prerelease.is_some()
    }

    /// The prerelease suffix, when present.
    pub fn prerelease(&self) -> Option<&str> {
        self.prerelease.as_deref()
    }

    /// The numeric components as written (`1.4` has two).
    pub fn components(&self) -> &[u64] {
        &self.components
    }
}

/// Drops a release-name prefix such as `TokenMon-` or `TokenMon v` when what
/// follows looks like a version.
fn strip_release_name(text: &str) -> &str {
    let starts_with_letter = text.chars().next().is_some_and(char::is_alphabetic);
    if !starts_with_letter {
        return text;
    }
    let run_end = text
        .char_indices()
        .find(|(_, c)| !c.is_alphanumeric())
        .map_or(text.len(), |(i, _)| i);
    let rest = &text[run_end..];
    let Some(sep) = rest.chars().next().filter(|c| matches!(c, '-' | '_' | ' ')) else {
        return text;
    };
    let after = rest[sep.len_utf8()..].trim_start_matches(' ');
    let digits_from = after.strip_prefix(['v', 'V']).unwrap_or(after);
    if digits_from
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit())
    {
        after
    } else {
        text
    }
}

impl fmt::Display for AppVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let nums = self
            .components
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(".");
        match &self.prerelease {
            Some(p) => write!(f, "{nums}-{p}"),
            None => write!(f, "{nums}"),
        }
    }
}

impl PartialEq for AppVersion {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for AppVersion {}

impl PartialOrd for AppVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AppVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        let len = self.components.len().max(other.components.len());
        for i in 0..len {
            let a = self.components.get(i).copied().unwrap_or(0);
            let b = other.components.get(i).copied().unwrap_or(0);
            match a.cmp(&b) {
                Ordering::Equal => {}
                unequal => return unequal,
            }
        }
        match (&self.prerelease, &other.prerelease) {
            (None, None) => Ordering::Equal,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(a), Some(b)) => compare_prerelease(a, b),
        }
    }
}

fn is_numeric(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
}

/// Numeric identifiers compare by value without overflowing on long digit runs.
fn compare_numeric(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn compare_prerelease(a: &str, b: &str) -> Ordering {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = match (is_numeric(x), is_numeric(y)) {
                    (true, true) => compare_numeric(x, y),
                    (true, false) => Ordering::Less,
                    (false, true) => Ordering::Greater,
                    (false, false) => x.cmp(y),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}
