//! Typed read access to each poller, kept beside the type-erased registry.
//!
//! The registry stores pollers as [`ManagedPoller`](crate::poller::ManagedPoller)
//! so the lifecycle code is provider-agnostic. Panels and the Overview chart
//! need each poller's own snapshot and hourly data; `build_registry` captures
//! the typed `Arc` in a closure per provider and registers it here, which keeps
//! the poller contract untouched and lets tests register plain closures.

use std::sync::Arc;

use std::fmt;

use chrono::{DateTime, Utc};
use tokenmon_core::ProviderId;
use tokenmon_core::overview::{CursorDay, DayHourly, OpenCodeDay};
use tokenmon_core::settings::Settings;
use tokenmon_core::usage::{Calendar, WeekOffset};
use tokenmon_provider_chatgpt::ChatGptPoller;
use tokenmon_provider_claude::poller::ClaudeUsagePoller;
use tokenmon_provider_cursor::poller::CursorUsagePoller;
use tokenmon_provider_grok::provider_poller::GrokProviderPoller;
use tokenmon_provider_grokbot::GrokbotUsagePoller;
use tokenmon_provider_opencode::poller::OpenCodeUsagePoller;
use tokenmon_provider_opencode::snapshot::{WindowKind, WindowUsage};
use tokenmon_provider_opencode::stats::{
    DayHourlyUsage, overview_hour_token_counts, overview_quota_hour_weights,
};
use tokenmon_provider_openrouter::OpenRouterUsagePoller;
use tokenmon_ui::panels::claude_chatgpt::{
    ChatGptSnapshotInput, ClaudeCodeInput, ClaudeSnapshotInput,
};
use tokenmon_ui::panels::grok::GrokSnapshotInput;
use tokenmon_ui::panels::grokbot_cursor::{CursorSnapshotInput, GrokbotSnapshotInput};
use tokenmon_ui::panels::opencode_openrouter::{OpenCodeSnapshotInput, OpenRouterSnapshotInput};

use tokenmon_core::Clock;
use tokenmon_provider_grok::models::ProductCatalog;

use crate::daily::{ChartContext, daily_input, grok_week_input, noun_for};

/// OpenCode's default monthly plan limit when no window carries one.
const DEFAULT_MONTHLY_LIMIT_USD: f64 = 60.0;

/// What a panel build asks of a source.
#[derive(Clone)]
pub struct ViewQuery<'a> {
    /// Current instant.
    pub now: DateTime<Utc>,
    /// Source of the local UTC offset at any instant (DST aware).
    pub clock: &'a dyn Clock,
    /// Browsed period, `0` current and negative earlier.
    pub week_offset: i32,
    /// "View all" is on for the models list.
    pub show_all_models: bool,
    /// The settings at build time (visible products, expanded sections).
    pub settings: Settings,
}

/// A provider's panel-ready snapshot input (absent before the first poll).
#[derive(Debug, Clone, PartialEq)]
pub enum SnapshotView {
    /// Grok.
    Grok(Option<GrokSnapshotInput>),
    /// Grokbot.
    Grokbot(Option<GrokbotSnapshotInput>),
    /// Cursor.
    Cursor(Option<CursorSnapshotInput>),
    /// Claude: the claude.ai snapshot and the Claude Code section.
    Claude(Option<ClaudeSnapshotInput>, Option<ClaudeCodeInput>),
    /// ChatGPT.
    ChatGpt(Option<ChatGptSnapshotInput>),
    /// OpenCode.
    OpenCode(Option<OpenCodeSnapshotInput>),
    /// OpenRouter.
    OpenRouter(Option<OpenRouterSnapshotInput>),
}

impl fmt::Debug for ViewQuery<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ViewQuery")
            .field("now", &self.now)
            .field("week_offset", &self.week_offset)
            .finish_non_exhaustive()
    }
}

/// Builds a provider's [`SnapshotView`].
pub type ViewSource = Arc<dyn Fn(&ViewQuery<'_>) -> SnapshotView + Send + Sync>;

/// One provider's contribution to today's Overview series.
#[derive(Debug, Clone, PartialEq)]
pub enum TodayPart {
    /// Grok hourly weights.
    Grok(DayHourly),
    /// Claude hourly weights.
    Claude(DayHourly),
    /// Grokbot hourly weights.
    Grokbot(DayHourly),
    /// Cursor quota and token hours.
    Cursor(CursorDay),
    /// OpenCode Go and Zen hours.
    OpenCode(OpenCodeDay),
}

/// Reads a provider's hourly data for the Overview.
pub type HourlySource = Arc<dyn Fn(&Calendar<'_>) -> Option<TodayPart> + Send + Sync>;

fn chart<'a>(
    provider: ProviderId,
    q: &ViewQuery<'_>,
    used: Option<f64>,
    period_start: Option<DateTime<Utc>>,
    resets_at: Option<DateTime<Utc>>,
    calendar: &'a Calendar<'a>,
) -> ChartContext<'a> {
    ChartContext {
        provider,
        noun: noun_for(provider),
        period_used_percent: used,
        period_start,
        resets_at,
        week_offset: q.week_offset,
        now: q.now,
        calendar,
    }
}

/// Grok: snapshot plus the per-product week.
pub fn grok_source(poller: &Arc<GrokProviderPoller>) -> ViewSource {
    let poller = Arc::clone(poller);
    Arc::new(move |q| {
        let usage = poller.usage();
        let Some(snapshot) = usage.snapshot() else {
            return SnapshotView::Grok(None);
        };
        let calendar = Calendar::local(q.clock);
        let ctx = chart(
            ProviderId::Grok,
            q,
            Some(snapshot.used_percent),
            None,
            snapshot.resets_at,
            &calendar,
        );
        let week = usage
            .week(i64::from(q.week_offset))
            .and_then(|w| grok_week_input(&w, &ctx));
        let ids: Vec<String> = q
            .settings
            .visible_product_ids
            .iter()
            .map(|p| p.as_str().to_owned())
            .collect();
        let visible = ProductCatalog::normalize_visible_ids(&ids);
        SnapshotView::Grok(Some(GrokSnapshotInput::from_snapshot(
            &snapshot, &visible, week,
        )))
    })
}

/// Grokbot: snapshot plus Monday-week bars.
pub fn grokbot_source(poller: &Arc<GrokbotUsagePoller>) -> ViewSource {
    let poller = Arc::clone(poller);
    Arc::new(move |q| {
        let Some(snapshot) = poller.snapshot() else {
            return SnapshotView::Grokbot(None);
        };
        let calendar = Calendar::local(q.clock);
        let ctx = chart(
            ProviderId::Grokbot,
            q,
            Some(snapshot.used_percent),
            None,
            snapshot.resets_at,
            &calendar,
        );
        let days = poller.daily_budget_days(WeekOffset::new(q.week_offset));
        let daily = daily_input(&days, &ctx);
        SnapshotView::Grokbot(Some(GrokbotSnapshotInput::from_snapshot(
            &snapshot, &calendar, daily,
        )))
    })
}

/// Cursor: snapshot plus the monthly-plan bars.
pub fn cursor_source(poller: &Arc<CursorUsagePoller>) -> ViewSource {
    let poller = Arc::clone(poller);
    Arc::new(move |q| {
        let Some(snapshot) = poller.snapshot() else {
            return SnapshotView::Cursor(None);
        };
        let calendar = Calendar::local(q.clock);
        let ctx = chart(
            ProviderId::Cursor,
            q,
            Some(snapshot.used_percent),
            snapshot.billing_cycle_start,
            snapshot.resets_at(),
            &calendar,
        );
        let days = poller
            .daily_budget_days_at(WeekOffset::new(q.week_offset))
            .unwrap_or_default();
        let mut input = CursorSnapshotInput::from(&snapshot);
        input.daily = daily_input(&days, &ctx);
        SnapshotView::Cursor(Some(input))
    })
}

/// Claude: snapshot, weekly bars and the Claude Code section.
pub fn claude_source(poller: &Arc<ClaudeUsagePoller>) -> ViewSource {
    let poller = Arc::clone(poller);
    Arc::new(move |q| {
        let code = poller.code_usage().map(|s| {
            ClaudeCodeInput::from_summary(
                &s,
                q.settings
                    .is_expanded(tokenmon_core::settings::PanelSection::ClaudeModels),
            )
        });
        let Some(snapshot) = poller.snapshot() else {
            return SnapshotView::Claude(None, code);
        };
        let calendar = Calendar::local(q.clock);
        let resets_at = poller.weekly_resets_at(q.now);
        let used = snapshot.seven_day.as_ref().map(|w| w.used_percent);
        let ctx = chart(ProviderId::Claude, q, used, None, resets_at, &calendar);
        let days = poller.daily_budget_days_at(WeekOffset::new(q.week_offset));
        let daily = daily_input(&days, &ctx);
        SnapshotView::Claude(
            Some(ClaudeSnapshotInput::from_snapshot(
                &snapshot, resets_at, daily,
            )),
            code,
        )
    })
}

/// ChatGPT: snapshot only.
pub fn chatgpt_source(poller: &Arc<ChatGptPoller>) -> ViewSource {
    let poller = Arc::clone(poller);
    Arc::new(move |_| SnapshotView::ChatGpt(poller.snapshot().as_ref().map(Into::into)))
}

/// OpenCode: snapshot plus monthly-plan bars.
pub fn opencode_source(poller: &Arc<OpenCodeUsagePoller>) -> ViewSource {
    let poller = Arc::clone(poller);
    Arc::new(move |q| {
        let Some(snapshot) = poller.snapshot() else {
            return SnapshotView::OpenCode(None);
        };
        let calendar = Calendar::local(q.clock);
        let monthly = snapshot
            .windows
            .iter()
            .find(|w| w.kind == WindowKind::Monthly);
        // Swift `monthlyUsedPercent`: monthly, else weekly, else 0.
        let used = monthly
            .or_else(|| {
                snapshot
                    .windows
                    .iter()
                    .find(|w| w.kind == WindowKind::Weekly)
            })
            .map_or(0.0, WindowUsage::used_percent);
        let ctx = chart(
            ProviderId::OpenCode,
            q,
            Some(used),
            poller.daily_budget_period_start(),
            monthly.and_then(|w| w.resets_at),
            &calendar,
        );
        let days = poller
            .daily_budget_days_at(WeekOffset::new(q.week_offset))
            .unwrap_or_default();
        SnapshotView::OpenCode(Some(OpenCodeSnapshotInput::from_snapshot(
            &snapshot,
            q.show_all_models,
            q.settings
                .is_expanded(tokenmon_core::settings::PanelSection::OpenCodeModels),
            daily_input(&days, &ctx),
        )))
    })
}

/// OpenRouter: snapshot only.
pub fn openrouter_source(poller: &Arc<OpenRouterUsagePoller>) -> ViewSource {
    let poller = Arc::clone(poller);
    Arc::new(move |q| {
        SnapshotView::OpenRouter(
            poller
                .snapshot()
                .map(|s| OpenRouterSnapshotInput::from_snapshot(&s, q.show_all_models)),
        )
    })
}

fn day_hourly(day_start: DateTime<Utc>, weights: &[f64], cal: &Calendar<'_>) -> DayHourly {
    DayHourly {
        day: cal.date_of(day_start),
        weights: weights.to_vec(),
    }
}

/// Claude's hourly weights.
pub fn claude_hourly(poller: &Arc<ClaudeUsagePoller>) -> HourlySource {
    let poller = Arc::clone(poller);
    Arc::new(move |cal| {
        let (start, weights) = poller.hourly_weights();
        Some(TodayPart::Claude(day_hourly(start, &weights, cal)))
    })
}

/// Cursor's quota and token hours.
pub fn cursor_hourly(poller: &Arc<CursorUsagePoller>) -> HourlySource {
    let poller = Arc::clone(poller);
    Arc::new(move |cal| {
        let usage = poller.day_hourly_usage()?;
        Some(TodayPart::Cursor(CursorDay {
            day: cal.date_of(usage.day_start),
            quota_weights: usage.quota_hour_weights.to_vec(),
            hour_tokens: usage.hour_token_weights.to_vec(),
        }))
    })
}

/// OpenCode's Go and Zen hours, as shares of the weekly slice of the monthly
/// limit.
pub fn opencode_hourly(poller: &Arc<OpenCodeUsagePoller>, clock: Arc<dyn Clock>) -> HourlySource {
    let poller = Arc::clone(poller);
    Arc::new(move |cal| {
        let usage = poller.day_hourly_usage()?;
        let limit = poller
            .snapshot()
            .and_then(|s| {
                s.windows
                    .iter()
                    .find(|w| w.kind == WindowKind::Monthly)
                    .map(|w| w.limit_usd)
            })
            .unwrap_or(DEFAULT_MONTHLY_LIMIT_USD);
        opencode_today_part(&usage, limit, clock.now(), cal)
    })
}

/// OpenCode's Overview part for `now`, or `None` when the stacks were built
/// for another local day (#68).
///
/// The poller's stacks go stale at local midnight until its next poll
/// rebuilds them (up to an hour when idle); stamping them with today's date
/// would record yesterday's hours as today's, and the history merges by max so
/// the wrong values would stay.
pub fn opencode_today_part(
    usage: &DayHourlyUsage,
    monthly_limit_usd: f64,
    now: DateTime<Utc>,
    cal: &Calendar<'_>,
) -> Option<TodayPart> {
    if !usage.is_for_day_of(now, cal) {
        return None;
    }
    let weights = overview_quota_hour_weights(usage, monthly_limit_usd);
    let tokens = overview_hour_token_counts(usage);
    let as_tokens = |v: [f64; 24]| v.iter().map(|t| *t as i64).collect::<Vec<_>>();
    Some(TodayPart::OpenCode(OpenCodeDay {
        day: usage.day,
        go_weights: weights.go.to_vec(),
        zen_weights: weights.zen.to_vec(),
        go_tokens: as_tokens(tokens.go),
        zen_tokens: as_tokens(tokens.zen),
    }))
}

/// Grokbot's hourly weights from its publisher's store.
pub fn grokbot_hourly(poller: &Arc<GrokbotUsagePoller>) -> HourlySource {
    let poller = Arc::clone(poller);
    Arc::new(move |cal| {
        let (start, weights) = poller.with_publisher(|p| {
            let h = p.hourly();
            (h.day_start(), h.hour_weights())
        });
        Some(TodayPart::Grokbot(day_hourly(start, &weights, cal)))
    })
}

/// Grok's hourly weights from the store the poller records into.
pub fn grok_hourly(store: &Arc<crate::providers::GrokHourly>) -> HourlySource {
    let store = Arc::clone(store);
    Arc::new(move |cal| {
        let (start, weights) = store.read();
        Some(TodayPart::Grok(day_hourly(start, &weights, cal)))
    })
}

/// Read and clear access to the Grok usage history (Settings export and the
/// "Clear history" button).
pub trait HistoryAccess: Send + Sync {
    /// Every recorded snapshot, oldest first.
    fn snapshots(&self) -> Vec<tokenmon_provider_grok::models::WeeklyUsageSnapshot>;
    /// Deletes the history.
    fn clear(&self);
    /// Whether the history file could not be read or written.
    fn store_failed(&self) -> bool;
    /// Whether rows changed since the last successful write.
    fn is_dirty(&self) -> bool;
    /// Writes the rows when they changed; a no-op when clean.
    fn flush(&self);
}

struct GrokHistory(Arc<GrokProviderPoller>);

impl HistoryAccess for GrokHistory {
    fn snapshots(&self) -> Vec<tokenmon_provider_grok::models::WeeklyUsageSnapshot> {
        self.0.usage().with_history(|h| h.all_snapshots().to_vec())
    }
    fn clear(&self) {
        self.0
            .usage()
            .with_history(tokenmon_provider_grok::history::GrokHistoryStore::clear);
    }
    fn store_failed(&self) -> bool {
        self.0.usage().with_history(|h| h.store_failed())
    }
    fn is_dirty(&self) -> bool {
        self.0.usage().with_history(|h| h.is_dirty())
    }
    fn flush(&self) {
        self.0.usage().flush_history();
    }
}

/// Grok's history behind [`HistoryAccess`].
pub fn grok_history(poller: &Arc<GrokProviderPoller>) -> Arc<dyn HistoryAccess> {
    Arc::new(GrokHistory(Arc::clone(poller)))
}

/// A history that holds nothing (registries without a Grok poller).
#[derive(Debug, Default)]
pub struct NoHistory;

impl HistoryAccess for NoHistory {
    fn snapshots(&self) -> Vec<tokenmon_provider_grok::models::WeeklyUsageSnapshot> {
        Vec::new()
    }
    fn clear(&self) {}
    fn store_failed(&self) -> bool {
        false
    }
    fn is_dirty(&self) -> bool {
        false
    }
    fn flush(&self) {}
}
