//! `Percent`, `DayKey`, `UsagePool`, `DailyTokenUsage`, `DailyBudget`, pace and `WeekOffset`.
//!
//! Owner: ws-core-usage.
//!
//! Everything here that depends on "which calendar day is it" takes a
//! [`Calendar`], which is backed by a fixed UTC offset or by an injected
//! [`Clock`](crate::Clock). The host time zone is never read implicitly.

pub mod budget;
pub mod calendar;
pub mod day_key;
pub mod pace;
pub mod percent;
pub mod pool;
pub mod tokens;
pub mod week_offset;

pub use budget::{DailyBudget, DailyBudgetDay, MonthBounds};
pub use calendar::Calendar;
pub use day_key::DayKey;
pub use pace::{AllowancePeriod, PaceHeadroom, ParseAllowancePeriodError};
pub use percent::Percent;
pub use pool::{ParseUsagePoolError, UsagePool};
pub use tokens::{
    DailyTokenUsage, DailyTokenUsageTally, ModelTokens, TokenCounts, attaching_tokens,
    attaching_tokens_map,
};
pub use week_offset::WeekOffset;
