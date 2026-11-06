//! Cookie selection and capture rules (spec 01 section 1.6.5).
//!
//! Pure functions over [`WebCookie`] values read from the browser cookie
//! manager. Nothing here touches the OS, so the allowlist, chunked-token and
//! e-mail rules are tested on every platform.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokenmon_core::AuthConfig;

/// Shown when capture finds no session and the config carries no message.
pub const DEFAULT_FAILURE_MESSAGE: &str =
    "Couldn't find a signed-in session yet. Finish signing in, then try again.";

/// One cookie as returned by `ICoreWebView2CookieManager::GetCookies`,
/// including `HttpOnly` ones (which JavaScript can never see).
#[derive(Clone, PartialEq)]
pub struct WebCookie {
    /// Cookie name (case preserved).
    pub name: String,
    /// Cookie value (secret).
    pub value: String,
    /// Domain attribute as reported by the engine (may start with a dot).
    pub domain: String,
    /// Path attribute.
    pub path: String,
    /// `HttpOnly` flag.
    pub http_only: bool,
    /// `Secure` flag.
    pub secure: bool,
    /// Expiry as Unix seconds; `None` for a session cookie.
    pub expires_unix: Option<f64>,
}

impl fmt::Debug for WebCookie {
    // Never print the value: it is a credential.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebCookie")
            .field("name", &self.name)
            .field("domain", &self.domain)
            .field("path", &self.path)
            .field("value", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl WebCookie {
    /// Convenience constructor for a host-only session cookie (tests, fakes).
    pub fn new(name: &str, value: &str, domain: &str) -> Self {
        Self {
            name: name.to_owned(),
            value: value.to_owned(),
            domain: domain.to_owned(),
            path: "/".to_owned(),
            http_only: false,
            secure: true,
            expires_unix: None,
        }
    }

    /// Whether the name and value can be placed in a `Cookie` header without
    /// altering its structure (no control characters, no `;` in the value, a
    /// non-empty token name). Cookies failing this are never captured, so a
    /// hostile page cannot smuggle extra header content into stored sessions.
    pub fn is_header_safe(&self) -> bool {
        !self.name.is_empty()
            && !self
                .name
                .bytes()
                .any(|b| b <= b' ' || b >= 0x7f || b"=;,\"\\()<>@:/[]?{}".contains(&b))
            && !self
                .value
                .bytes()
                .any(|b| b < b' ' || b == 0x7f || b == b';')
    }
}

/// Predicate over a cookie domain or name.
pub type Predicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// `WebKitCookieCapture.Policy` for one provider.
#[derive(Clone)]
pub struct CapturePolicy {
    /// Whether a cookie domain belongs to the provider (`Domain::matches`).
    pub is_domain: Predicate,
    /// Provider auth settings: allowlist, preferred cookie, attempts, delay.
    pub auth: AuthConfig,
    /// Heuristic for providers without an allowlist (`looksLikeAuthCookie`).
    pub looks_like_auth_cookie: Predicate,
    /// Return every provider-domain cookie once a session cookie is found.
    pub include_all_domain_cookies_when_session_found: bool,
}

impl fmt::Debug for CapturePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapturePolicy")
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

impl CapturePolicy {
    /// Policy from an [`AuthConfig`]; `looks_like_auth_cookie` defaults to
    /// "never" (the allowlist providers do not need it).
    pub fn new(auth: AuthConfig, is_domain: Predicate) -> Self {
        Self {
            is_domain,
            auth,
            looks_like_auth_cookie: Arc::new(|_| false),
            include_all_domain_cookies_when_session_found: true,
        }
    }

    /// Capture attempts, at least 1.
    pub fn max_attempts(&self) -> u32 {
        self.auth.max_attempts.max(1)
    }

    /// Delay between attempts.
    pub fn retry_delay(&self) -> Duration {
        self.auth.retry_delay
    }

    /// Message shown when no session was found.
    pub fn failure_message(&self) -> &str {
        if self.auth.failure_message.is_empty() {
            DEFAULT_FAILURE_MESSAGE
        } else {
            &self.auth.failure_message
        }
    }
}

/// A successful capture.
#[derive(Clone, PartialEq)]
pub struct CapturedCookies {
    /// The selected cookies.
    pub cookies: Vec<WebCookie>,
    /// `name=value` pairs joined with `"; "` (secret).
    pub cookie_header: String,
    /// Plausible account e-mail found in the cookies.
    pub email: Option<String>,
}

impl fmt::Debug for CapturedCookies {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapturedCookies")
            .field("cookies", &self.cookies.len())
            .field("cookie_header", &"<redacted>")
            .field("email", &self.email)
            .finish()
    }
}

/// The preferred session cookie, or one of its `<name>.N` chunks: NextAuth
/// splits big tokens into `<name>.0`, `<name>.1`, ... and a chunked-only
/// login is a valid session.
fn is_session_cookie(policy: &CapturePolicy, name: &str) -> bool {
    policy.auth.is_preferred_session_cookie(name)
        || name
            .to_lowercase()
            .strip_prefix(policy.auth.preferred_session_cookie.as_str())
            .is_some_and(|rest| {
                rest.strip_prefix('.')
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
}

/// `select(cookies, policy)`: the cookies to keep, or `None` when there is no
/// signed-in session yet.
pub fn select(cookies: &[WebCookie], policy: &CapturePolicy) -> Option<Vec<WebCookie>> {
    let relevant: Vec<&WebCookie> = cookies
        .iter()
        .filter(|c| c.is_header_safe() && (policy.is_domain)(&c.domain))
        .collect();
    if relevant.is_empty() {
        return None;
    }
    if policy.auth.has_essential_allowlist() {
        let essential: Vec<&WebCookie> = relevant
            .into_iter()
            .filter(|c| policy.auth.is_essential(&c.name))
            .collect();
        // A non-empty value is required: a blank session cookie is not a
        // session.
        let has_session = essential
            .iter()
            .any(|c| is_session_cookie(policy, &c.name) && !c.value.is_empty());
        return has_session.then(|| essential.into_iter().cloned().collect());
    }
    let preferred = relevant.iter().find(|c| is_session_cookie(policy, &c.name));
    let authish: Vec<&WebCookie> = relevant
        .iter()
        .copied()
        .filter(|c| (policy.looks_like_auth_cookie)(&c.name))
        .collect();
    let include_all = policy.include_all_domain_cookies_when_session_found;
    if let Some(p) = preferred {
        return Some(if include_all {
            relevant.into_iter().cloned().collect()
        } else {
            vec![(*p).clone()]
        });
    }
    if !authish.is_empty() {
        return Some(if include_all {
            relevant.into_iter().cloned().collect()
        } else {
            authish.into_iter().cloned().collect()
        });
    }
    None
}

/// `name=value` pairs joined with `"; "`.
pub fn cookie_header(cookies: &[WebCookie]) -> String {
    cookies
        .iter()
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ")
}

/// One capture attempt: [`select`] plus header and e-mail.
pub fn capture(cookies: &[WebCookie], policy: &CapturePolicy) -> Option<CapturedCookies> {
    let selected = select(cookies, policy)?;
    Some(CapturedCookies {
        cookie_header: cookie_header(&selected),
        email: extract_email(&selected),
        cookies: selected,
    })
}

/// `extractEmail`: cookies named `email` / `user_email` first, then any cookie
/// whose percent-decoded value is a plausible e-mail address.
pub fn extract_email(cookies: &[WebCookie]) -> Option<String> {
    let by_name = cookies.iter().find_map(|c| {
        let n = c.name.to_lowercase();
        if n == "email" || n == "user_email" {
            let v = percent_decode(&c.value);
            is_plausible_email(&v).then_some(v)
        } else {
            None
        }
    });
    by_name.or_else(|| {
        cookies.iter().find_map(|c| {
            let v = percent_decode(&c.value);
            is_plausible_email(&v).then_some(v)
        })
    })
}

/// Percent-decodes `%XX` sequences; invalid escapes are kept literally and the
/// result is lossy UTF-8.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hi = (b[i + 1] as char).to_digit(16);
            let lo = (b[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `isPlausibleEmail` (spec 1.6.5).
pub fn is_plausible_email(v: &str) -> bool {
    if v.chars().count() > 254 {
        return false;
    }
    let mut parts = v.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let local_ok = (1..=64).contains(&local.chars().count())
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".!#$%&'*+-/=?^_`{|}~".contains(c));
    if !local_ok
        || !domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2
        || labels
            .iter()
            .any(|l| l.is_empty() || l.starts_with('-') || l.ends_with('-'))
    {
        return false;
    }
    let tld = labels[labels.len() - 1];
    tld.chars().count() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())
}

/// Identifies one cookie for deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieRef {
    /// Cookie name.
    pub name: String,
    /// Domain attribute.
    pub domain: String,
    /// Path attribute.
    pub path: String,
}

/// Scoped sign-out: the cookies of `cookies` that belong to the provider's
/// domains. Cookies of other domains are never touched.
pub fn scoped_deletions(cookies: &[WebCookie], is_domain: &dyn Fn(&str) -> bool) -> Vec<CookieRef> {
    cookies
        .iter()
        .filter(|c| is_domain(&c.domain))
        .map(|c| CookieRef {
            name: c.name.clone(),
            domain: c.domain.clone(),
            path: c.path.clone(),
        })
        .collect()
}
