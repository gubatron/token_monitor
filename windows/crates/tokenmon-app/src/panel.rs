//! Panel content: builds the Overview view-model input from live model state
//! (spec 04 section 3.5). Portable; the Slint window lives in `shell`.

use chrono::{DateTime, FixedOffset, Utc};
use tokenmon_ui::panels::common::PanelContext;
use tokenmon_ui::panels::overview::{
    OverviewCardInput, OverviewDailyInput, OverviewInput, OverviewPanelVm, build_overview,
};

use crate::model::AppModel;
use crate::sessions::Sessions;

/// Hours in a day, the width of the daily chart.
const HOURS: usize = 24;

/// One Overview card input per visible provider, in the user's order.
pub fn card_inputs(model: &AppModel, sessions: &Sessions) -> Vec<OverviewCardInput> {
    let settings = model.settings().snapshot();
    settings
        .visible_usage_providers()
        .into_iter()
        .filter_map(|p| {
            let poller = &model.registry().entry(p)?.poller;
            let percent = poller.headline_used_percent();
            let state = sessions.signed_in(p);
            Some(OverviewCardInput {
                provider: p,
                has_snapshot: percent.is_some(),
                percent,
                reset_at: poller.headline_resets_at(),
                signed_in: state,
                needs_sign_in: !state,
                key_usage_usd: None,
                has_budget_source: false,
                remaining_usd: None,
            })
        })
        .collect()
}

/// The Overview panel's view model for `now`.
///
/// The daily chart is empty until the hourly history is wired; every card is
/// live.
pub fn overview(
    model: &AppModel,
    sessions: &Sessions,
    version: &str,
    now: DateTime<Utc>,
    tz: FixedOffset,
) -> OverviewPanelVm {
    build_overview(
        &OverviewInput {
            version: Some(version.to_owned()),
            cards: card_inputs(model, sessions),
            daily: OverviewDailyInput {
                day_offset: 0,
                hours: vec![Vec::new(); HOURS],
                earliest_day: None,
            },
        },
        &PanelContext { now, tz },
    )
}
