//! Cookie/host domain matching (spec 01 section 1.1.5).

/// Namespace for [`Domain::matches`].
#[derive(Debug, Clone, Copy)]
pub struct Domain;

impl Domain {
    /// True when `domain` (cookie-style, so possibly `.claude.ai`) equals one of
    /// `hosts` or is a subdomain of it.
    ///
    /// The domain is lowercased and stripped of leading and trailing dots first.
    /// `hosts` are compared as given (providers pass lowercase literals).
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::net::Domain;
    ///
    /// assert!(Domain::matches(".Claude.AI.", &["claude.ai"]));
    /// assert!(Domain::matches("api.claude.ai", &["claude.ai"]));
    /// assert!(!Domain::matches("notclaude.ai", &["claude.ai"]));
    /// assert!(!Domain::matches("claude.ai.evil.com", &["claude.ai"]));
    /// ```
    pub fn matches(domain: &str, hosts: &[&str]) -> bool {
        let normalized = domain.to_lowercase();
        let d = normalized.trim_matches('.');
        hosts
            .iter()
            .any(|host| d == *host || d.strip_suffix(host).is_some_and(|rest| rest.ends_with('.')))
    }
}
