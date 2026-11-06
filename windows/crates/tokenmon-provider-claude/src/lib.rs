//! TokenMon provider: Claude.
//!
//! Two independent data sources feed the Claude panel:
//!
//! * claude.ai rate-limit windows ([`usage`]): the usage endpoint's JSON, the
//!   organization id taken from the session cookie header, and the snapshot
//!   model.
//! * Claude Code local transcripts: discovery of `projects` directories from
//!   an injected [`discovery::ClaudeEnvironment`], the JSONL line parser
//!   ([`line`]), the incremental file cache with per-calendar tallies
//!   ([`cache`]), the forever-ledger ([`ledger`]), the month/all-time summary
//!   ([`summary`]), API pricing ([`pricing`]) and the daily-bar data with
//!   per-day hover tokens ([`budget`]). Sign-in rules live in [`auth`].
//!
//! Everything here is portable and hermetic: no function reads the real home
//! directory or process environment on its own.

pub mod auth;
pub mod budget;
pub mod cache;
pub mod discovery;
pub mod ledger;
pub mod line;
pub mod pricing;
pub mod summary;
pub mod tokens;
pub mod usage;

pub use cache::ClaudeCodeFileCache;
pub use discovery::{ClaudeEnvironment, projects_directories, scan_daily_tokens};
pub use ledger::ClaudeCodeUsageLedger;
pub use line::{Entry, parse_complete_lines, parse_jsonl, parse_line, usage_line_ranges};
pub use pricing::{ClaudeModelRates, base_model_id, cost, display_name, rates};
pub use summary::{ClaudeCodeUsageSummary, ModelUsageRow, UsageTotals};
pub use tokens::{DailyTokens, DayCalendar, DayKey, ModelTokens};
pub use usage::{
    ClaudeSnapshot, ClaudeUsageResponse, ClaudeUsageWindow, organization_id_from_cookie_header,
    parse_usage_body, project_weekly_reset,
};
