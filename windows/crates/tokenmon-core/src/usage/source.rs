//! Where a usage figure was counted: the provider's account or this PC.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Where a usage figure was counted: the provider's account, or this
/// computer's own logs.
///
/// Figures read from local files (Claude Code transcripts, OpenCode's
/// `opencode.db`) cover only what ran on this PC, so the panel labels them
/// "This PC" to keep them from being read as account-wide totals. Account
/// figures (the 5-hour and weekly pools, the daily bars' percentages,
/// Cursor's and OpenRouter's server numbers) carry no label.
///
/// Every label string comes from here, so the wording changes in one place.
/// Types that carry counts take a `UsageSource` with no default, so a new
/// provider has to say where its counts come from instead of silently going
/// unlabelled.
///
/// # Examples
///
/// ```
/// use tokenmon_core::usage::UsageSource;
/// assert_eq!(UsageSource::ThisComputer.tag().as_deref(), Some("This PC"));
/// assert_eq!(UsageSource::ThisComputer.tokens_unit(), "tokens on this PC");
/// assert_eq!(UsageSource::Account.tokens_unit(), "tokens");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageSource {
    /// Reported by the provider for the whole account, across every device.
    Account,
    /// Counted from this computer's local logs only.
    ThisComputer,
}

/// Error returned when parsing an unknown [`UsageSource`] string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown usage source: {0:?}")]
pub struct ParseUsageSourceError(pub String);

impl UsageSource {
    /// Every source, for exhaustive tests.
    pub const ALL: [Self; 2] = [Self::Account, Self::ThisComputer];

    /// What the labels call this computer (the macOS app says "Mac").
    pub const COMPUTER_NAME: &'static str = "PC";

    /// The stable string form (`account`, `thisComputer`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::ThisComputer => "thisComputer",
        }
    }

    /// The short tag on a section of local figures, "This PC"; `None` for
    /// account figures, which are not labelled.
    #[must_use]
    pub fn tag(self) -> Option<String> {
        match self {
            Self::Account => None,
            Self::ThisComputer => Some(format!("This {}", Self::COMPUTER_NAME)),
        }
    }

    /// The phrase after a local figure, "on this PC"; `None` for account
    /// figures.
    #[must_use]
    pub fn location_phrase(self) -> Option<String> {
        match self {
            Self::Account => None,
            Self::ThisComputer => Some(format!("on this {}", Self::COMPUTER_NAME)),
        }
    }

    /// The words after a token count: "tokens", or "tokens on this PC".
    #[must_use]
    pub fn tokens_unit(self) -> String {
        self.qualifying("tokens")
    }

    /// `text` followed by [`location_phrase`](Self::location_phrase) when
    /// there is one, e.g. "Tokens this month on this PC"; screen readers
    /// speak local sections this way.
    #[must_use]
    pub fn qualifying(self, text: &str) -> String {
        match self.location_phrase() {
            Some(phrase) => format!("{text} {phrase}"),
            None => text.to_owned(),
        }
    }
}

impl fmt::Display for UsageSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for UsageSource {
    type Err = ParseUsageSourceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|v| v.as_str() == s)
            .ok_or_else(|| ParseUsageSourceError(s.to_owned()))
    }
}
