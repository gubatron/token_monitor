//! Month-over-month change of a model's spend or tokens, and its one-line
//! wording (Swift `MonthlyUsageChange`).

use crate::util::Format;

/// What the compared value measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeUnit {
    /// API-rate value in US dollars.
    Usd,
    /// Token counts.
    Tokens,
}

impl ChangeUnit {
    /// Every unit, in declaration order.
    pub const ALL: [Self; 2] = [Self::Usd, Self::Tokens];

    /// The smallest difference shown in this unit: half a cent, or half a
    /// token; anything smaller rounds away and counts as no change.
    #[must_use]
    pub fn resolution(self) -> f64 {
        match self {
            Self::Usd => 0.005,
            Self::Tokens => 0.5,
        }
    }

    /// The stable name (`usd`, `tokens`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Usd => "usd",
            Self::Tokens => "tokens",
        }
    }

    /// `amount` (non-negative) as shown in a change, e.g. `$212.40` or
    /// `1.2M tokens`.
    #[must_use]
    pub fn format(self, amount: f64) -> String {
        match self {
            Self::Usd => Format::usd(amount),
            Self::Tokens => format!("{} tokens", Format::tokens(amount.round() as i64)),
        }
    }
}

/// An unknown unit name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown change unit `{0}`")]
pub struct ParseChangeUnitError(pub String);

impl std::str::FromStr for ChangeUnit {
    type Err = ParseChangeUnitError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "usd" => Ok(Self::Usd),
            "tokens" => Ok(Self::Tokens),
            other => Err(ParseChangeUnitError(other.to_owned())),
        }
    }
}

/// How one value moved from last month to this month.
///
/// # Examples
///
/// ```
/// use tokenmon_core::usage::{ChangeUnit, MonthlyUsageChange};
/// let change = MonthlyUsageChange::between(Some(1_500.05), 1_712.45, ChangeUnit::Usd);
/// assert_eq!(change.summary("October", ChangeUnit::Usd), "↑ 14% · +$212.40 vs October");
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MonthlyUsageChange {
    /// No usage last month, so there is nothing to compare against.
    New,
    /// The two months differ by less than the unit's resolution.
    Unchanged,
    /// Up by `amount`; `percent` is `None` when last month's value was too
    /// small to divide by (it rounds to zero in the unit).
    Increase {
        /// The rise, positive.
        amount: f64,
        /// The rise as a percentage of last month.
        percent: Option<f64>,
    },
    /// Down by `amount` (a positive number); `percent` as for `Increase`.
    Decrease {
        /// The drop, positive.
        amount: f64,
        /// The drop as a positive percentage of last month.
        percent: Option<f64>,
    },
}

impl MonthlyUsageChange {
    /// The change from `previous` to `current`; `previous` is `None` when
    /// there was no usage at all last month (a model first used this month).
    #[must_use]
    pub fn between(previous: Option<f64>, current: f64, unit: ChangeUnit) -> Self {
        let Some(previous) = previous else {
            return Self::New;
        };
        let difference = current - previous;
        if difference.abs() < unit.resolution() {
            return Self::Unchanged;
        }
        let percent = (previous >= unit.resolution()).then(|| difference / previous * 100.0);
        if difference > 0.0 {
            Self::Increase {
                amount: difference,
                percent,
            }
        } else {
            Self::Decrease {
                amount: -difference,
                percent: percent.map(f64::abs),
            }
        }
    }

    /// One line describing the change against `previous_month` (its name,
    /// e.g. `October`): `↑ 18% · +$212.40 vs October`, `↓ 12% · −$80.00 vs
    /// October`, `↑ $3.91 vs October` when last month rounds to zero,
    /// `Same as October`, or `New this month`.
    #[must_use]
    pub fn summary(&self, previous_month: &str, unit: ChangeUnit) -> String {
        match *self {
            Self::New => "New this month".to_owned(),
            Self::Unchanged => format!("Same as {previous_month}"),
            Self::Increase { amount, percent } => {
                line("↑", "+", amount, percent, unit, previous_month)
            }
            Self::Decrease { amount, percent } => {
                line("↓", "\u{2212}", amount, percent, unit, previous_month)
            }
        }
    }

    /// [`Self::summary`] in words for a screen reader, without arrows or
    /// signs: `Up 18%, $212.40 more than October`, `$3.91 more than October`,
    /// `Down 12%, $80.00 less than October`, `Same as October`, `New this
    /// month`.
    #[must_use]
    pub fn spoken_summary(&self, previous_month: &str, unit: ChangeUnit) -> String {
        match *self {
            Self::New | Self::Unchanged => self.summary(previous_month, unit),
            Self::Increase { amount, percent } => {
                let difference = format!("{} more than {previous_month}", unit.format(amount));
                percent.map_or(difference.clone(), |p| {
                    format!("Up {}, {difference}", Self::percent_text(p))
                })
            }
            Self::Decrease { amount, percent } => {
                let difference = format!("{} less than {previous_month}", unit.format(amount));
                percent.map_or(difference.clone(), |p| {
                    format!("Down {}, {difference}", Self::percent_text(p))
                })
            }
        }
    }

    /// A percentage rounded to a whole number, without overstating the
    /// extremes: `<1%` for a change that rounds to 0, and `>99%` for one just
    /// short of 100 (a drop to nearly, but not exactly, nothing).
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::usage::MonthlyUsageChange;
    /// assert_eq!(MonthlyUsageChange::percent_text(0.3), "<1%");
    /// assert_eq!(MonthlyUsageChange::percent_text(99.7), ">99%");
    /// assert_eq!(MonthlyUsageChange::percent_text(-25.0), "25%");
    /// ```
    #[must_use]
    pub fn percent_text(percent: f64) -> String {
        let magnitude = percent.abs();
        let rounded = magnitude.round() as i64;
        if rounded == 0 {
            "<1%".to_owned()
        } else if rounded == 100 && magnitude < 100.0 {
            ">99%".to_owned()
        } else {
            format!("{rounded}%")
        }
    }
}

fn line(
    arrow: &str,
    sign: &str,
    amount: f64,
    percent: Option<f64>,
    unit: ChangeUnit,
    previous_month: &str,
) -> String {
    match percent {
        None => format!("{arrow} {} vs {previous_month}", unit.format(amount)),
        Some(p) => format!(
            "{arrow} {} · {sign}{} vs {previous_month}",
            MonthlyUsageChange::percent_text(p),
            unit.format(amount)
        ),
    }
}
