//! Cadence of the quota pool a provider's main usage card measures.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Cadence of a quota pool, rendered as the card header (`Usage Pool Weekly`).
///
/// A provider that exposes one cadence keeps its name; two or more collapse to
/// [`UsagePool::Mixed`].
///
/// # Examples
///
/// ```
/// use tokenmon_core::usage::UsagePool;
/// assert_eq!(UsagePool::combining(&[UsagePool::Weekly, UsagePool::Weekly]), UsagePool::Weekly);
/// assert_eq!(UsagePool::combining(&[]), UsagePool::Mixed);
/// assert_eq!(UsagePool::Hourly.section_title(), "Usage Pool Hourly");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsagePool {
    /// A short rolling window (for example 5 hours).
    Hourly,
    /// A weekly window.
    Weekly,
    /// A monthly / billing-cycle window.
    Monthly,
    /// Two or more cadences.
    Mixed,
}

/// Error returned when parsing an unknown [`UsagePool`] string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown usage pool: {0:?}")]
pub struct ParseUsagePoolError(pub String);

impl UsagePool {
    /// Human-readable name: Hourly, Weekly, Monthly or Mixed.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Hourly => "Hourly",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
            Self::Mixed => "Mixed",
        }
    }

    /// Section-header text, `"Usage Pool <display name>"`.
    #[must_use]
    pub fn section_title(self) -> String {
        format!("Usage Pool {}", self.display_name())
    }

    /// Collapses the cadences a provider exposes: exactly one unique value keeps
    /// its name; none or several give [`UsagePool::Mixed`].
    #[must_use]
    pub fn combining(pools: &[UsagePool]) -> UsagePool {
        let unique: HashSet<UsagePool> = pools.iter().copied().collect();
        let mut it = unique.into_iter();
        match (it.next(), it.next()) {
            (Some(only), None) => only,
            _ => Self::Mixed,
        }
    }
}

impl fmt::Display for UsagePool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Hourly => "hourly",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Mixed => "mixed",
        })
    }
}

impl FromStr for UsagePool {
    type Err = ParseUsagePoolError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "hourly" => Ok(Self::Hourly),
            "weekly" => Ok(Self::Weekly),
            "monthly" => Ok(Self::Monthly),
            "mixed" => Ok(Self::Mixed),
            other => Err(ParseUsagePoolError(other.to_owned())),
        }
    }
}
