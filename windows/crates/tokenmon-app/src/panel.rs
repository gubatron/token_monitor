//! Panel content: builds every panel's view model from live model state and
//! turns the user's panel actions into app actions (spec 04 section 3).
//!
//! Portable; the Slint windows live in `shell`. [`PanelState`] holds what the
//! panels remember between redraws (selected tab, browsed period, "View all",
//! the OpenRouter key draft); [`PanelState::handle`] is the pure reducer from
//! a [`PanelEvent`] to the [`Action`]s the shell executes.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use tokenmon_core::settings::{PanelSection, Settings};
use tokenmon_core::usage::Calendar;
use tokenmon_core::{Clock, ProviderId};
use tokenmon_ui::panels::claude_chatgpt::{
    ChatGptInput, ClaudeInput, ClaudePanelVm, build_chatgpt, build_claude,
};
use tokenmon_ui::panels::common::{
    PanelContext, PanelVm, ProviderStatus, week_next, week_previous,
};
use tokenmon_ui::panels::events::PanelEvent;
use tokenmon_ui::panels::grok::{GrokInput, GrokUsage, build as build_grok};
use tokenmon_ui::panels::grokbot_cursor::{
    CursorInput, CursorUsage, GrokbotInput, GrokbotUsage, build_cursor, build_grokbot,
};
use tokenmon_ui::panels::opencode_openrouter::{
    OpenCodeInput, OpenCodeUsage, OpenRouterInput, OpenRouterPanelVm, build_opencode,
    build_openrouter,
};
use tokenmon_ui::panels::overview::{
    CardAction, OverviewCardInput, OverviewCardVm, OverviewInput, OverviewPanelVm, build_overview,
};
use tokenmon_ui::panels::switcher::{RowVm, tab_provider};

use crate::model::AppModel;
use crate::overview::{OverviewStore, today_series};
use crate::sessions::Sessions;
use crate::sources::{SnapshotView, ViewQuery};

/// Everything the panels read.
pub struct ViewEnv<'a> {
    /// The lifecycle model (registry and settings).
    pub model: &'a AppModel,
    /// Credential sessions.
    pub sessions: &'a Sessions,
    /// Overview history.
    pub overview: &'a OverviewStore,
    /// App version for the Overview header.
    pub version: &'a str,
    /// Current instant.
    pub now: DateTime<Utc>,
    /// Local offset source (DST aware).
    pub clock: &'a dyn Clock,
}

/// One provider's panel, built and ready to apply to its window.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelView {
    /// The Overview.
    Overview(OverviewPanelVm),
    /// Grok.
    Grok(PanelVm<GrokUsage>),
    /// Grokbot.
    Grokbot(PanelVm<GrokbotUsage>),
    /// Cursor.
    Cursor(PanelVm<CursorUsage>),
    /// Claude.
    Claude(ClaudePanelVm),
    /// ChatGPT.
    ChatGpt(PanelVm<tokenmon_ui::panels::claude_chatgpt::ChatGptUsage>),
    /// OpenCode.
    OpenCode(PanelVm<OpenCodeUsage>),
    /// OpenRouter.
    OpenRouter(OpenRouterPanelVm),
}

impl PanelView {
    /// The provider whose body this view fills.
    pub fn provider(&self) -> ProviderId {
        match self {
            Self::Overview(_) => ProviderId::Overview,
            Self::Grok(_) => ProviderId::Grok,
            Self::Grokbot(_) => ProviderId::Grokbot,
            Self::Cursor(_) => ProviderId::Cursor,
            Self::Claude(_) => ProviderId::Claude,
            Self::ChatGpt(_) => ProviderId::ChatGpt,
            Self::OpenCode(_) => ProviderId::OpenCode,
            Self::OpenRouter(_) => ProviderId::OpenRouter,
        }
    }
}

/// What the shell does after a panel action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Rebuild and re-apply the current panel.
    Redraw,
    /// Switch to this tab.
    Show(ProviderId),
    /// Open the Settings window.
    OpenSettings,
    /// Open this provider's sign-in window.
    SignIn(ProviderId),
    /// Sign this provider out.
    SignOut(ProviderId),
    /// Flip a collapsible section and redraw.
    ToggleSection(PanelSection),
    /// Store this OpenRouter key.
    SubmitApiKey(String),
}

/// What the panels remember between redraws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelState {
    /// The tab on screen.
    pub selected: ProviderId,
    offsets: BTreeMap<ProviderId, i32>,
    show_all: BTreeSet<ProviderId>,
    /// Text typed in the OpenRouter key field.
    pub key_draft: String,
    /// `Replace API Key…` was chosen.
    pub replacing_key: bool,
    /// The last key-save failure.
    pub key_error: Option<String>,
}

impl PanelState {
    /// Starts on `selected`.
    pub fn new(selected: ProviderId) -> Self {
        Self {
            selected,
            offsets: BTreeMap::new(),
            show_all: BTreeSet::new(),
            key_draft: String::new(),
            replacing_key: false,
            key_error: None,
        }
    }

    /// Returns every chart to its current period (the day rolled over, so
    /// "today" is a different date).
    pub fn reset_offsets(&mut self) {
        self.offsets.clear();
    }

    /// The browsed period of `provider` (`0` current, negative earlier).
    pub fn offset(&self, provider: ProviderId) -> i32 {
        self.offsets.get(&provider).copied().unwrap_or(0)
    }

    /// Whether the models list shows every row for `provider`.
    pub fn shows_all(&self, provider: ProviderId) -> bool {
        self.show_all.contains(&provider)
    }

    /// Reducer from a panel event to the actions the shell runs. `rows` is the
    /// applied switcher grid and `cards` the applied Overview cards (events
    /// carry raw indices).
    pub fn handle(
        &mut self,
        event: &PanelEvent,
        rows: &[RowVm],
        cards: &[OverviewCardVm],
    ) -> Vec<Action> {
        match event {
            PanelEvent::OpenSettings => vec![Action::OpenSettings],
            PanelEvent::SelectTab { row, col } => tab_provider(rows, *row, *col)
                .map(Action::Show)
                .into_iter()
                .collect(),
            PanelEvent::SignIn(p) | PanelEvent::SignInAgain(p) => vec![Action::SignIn(*p)],
            PanelEvent::SignOut(p) => vec![Action::SignOut(*p)],
            PanelEvent::PreviousPeriod(p) => self.page(*p, week_previous),
            PanelEvent::NextPeriod(p) => self.page(*p, week_next),
            PanelEvent::ToggleModelsSection(p) => {
                section(*p).map(Action::ToggleSection).into_iter().collect()
            }
            PanelEvent::ToggleShowAllModels(p) => {
                if !self.show_all.remove(p) {
                    self.show_all.insert(*p);
                }
                vec![Action::Redraw]
            }
            PanelEvent::OverviewSignIn(i) => cards
                .get(*i)
                .and_then(|c| c.sign_in.as_ref())
                .map(|(_, a)| match a {
                    CardAction::OpenSignIn(p) => Action::SignIn(*p),
                    CardAction::SelectTab(p) => Action::Show(*p),
                })
                .into_iter()
                .collect(),
            PanelEvent::OverviewSignOut(i) => cards
                .get(*i)
                .and_then(|c| c.sign_out)
                .map(Action::SignOut)
                .into_iter()
                .collect(),
            PanelEvent::ApiKeyEdited(text) => {
                self.key_draft.clone_from(text);
                Vec::new()
            }
            PanelEvent::SubmitApiKey(key) => vec![Action::SubmitApiKey(key.clone())],
            PanelEvent::ReplaceApiKey => {
                self.replacing_key = true;
                vec![Action::Redraw]
            }
        }
    }

    fn page(&mut self, provider: ProviderId, step: fn(i32) -> i32) -> Vec<Action> {
        let next = step(self.offset(provider));
        self.offsets.insert(provider, next);
        vec![Action::Redraw]
    }

    /// Records the outcome of storing an OpenRouter key: success leaves the
    /// replace flow and clears the field, failure keeps the draft and shows
    /// the reason.
    pub fn key_saved(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.key_draft.clear();
                self.replacing_key = false;
                self.key_error = None;
            }
            Err(message) => self.key_error = Some(message),
        }
    }
}

fn section(provider: ProviderId) -> Option<PanelSection> {
    match provider {
        ProviderId::Claude => Some(PanelSection::ClaudeModels),
        ProviderId::OpenCode => Some(PanelSection::OpenCodeModels),
        _ => None,
    }
}

/// Stores an OpenRouter key through the API-key session and records the result
/// in `state`. Returns whether the key was accepted.
pub fn commit_api_key(state: &mut PanelState, sessions: &Sessions, key: &str) -> bool {
    let result = sessions
        .openrouter
        .save_api_key(key)
        .map_err(|e| e.to_string());
    let ok = result.is_ok();
    state.key_saved(result);
    ok
}

fn status(env: &ViewEnv<'_>, provider: ProviderId) -> ProviderStatus {
    let auth = env.sessions.auth_state(provider);
    let poller = env.model.registry().entry(provider).map(|e| &e.poller);
    ProviderStatus {
        needs_sign_in: auth.needs_sign_in,
        has_auth_error: auth.last_auth_error.is_some(),
        is_refreshing: false,
        last_error: env
            .sessions
            .sign_in_error(provider)
            .filter(|_| !auth.is_signed_in)
            .or_else(|| poller.and_then(|p| p.last_error()).map(|e| e.to_string())),
        last_refreshed_at: poller.and_then(|p| p.last_refreshed_at()),
    }
}

fn snapshot_view(
    env: &ViewEnv<'_>,
    state: &PanelState,
    settings: &Settings,
    provider: ProviderId,
) -> Option<SnapshotView> {
    let source = env.model.registry().view(provider)?;
    Some(source(&ViewQuery {
        now: env.now,
        clock: env.clock,
        week_offset: state.offset(provider),
        show_all_models: state.shows_all(provider),
        settings: settings.clone(),
    }))
}

/// One Overview card input per visible provider, in the user's order.
pub fn card_inputs(
    model: &AppModel,
    sessions: &Sessions,
    openrouter: Option<(Option<f64>, bool, Option<f64>)>,
) -> Vec<OverviewCardInput> {
    let settings = model.settings().snapshot();
    settings
        .visible_usage_providers()
        .into_iter()
        .filter_map(|p| {
            let poller = &model.registry().entry(p)?.poller;
            let percent = poller.overview_used_percent();
            let state = sessions.signed_in(p);
            let facts = openrouter.filter(|_| p == ProviderId::OpenRouter);
            let (key_usage_usd, has_budget_source, remaining_usd) =
                facts.unwrap_or((None, false, None));
            Some(OverviewCardInput {
                provider: p,
                // An OpenRouter key without a credit limit has a snapshot but no
                // percent (Swift shows its spend, not "No OpenRouter data yet").
                has_snapshot: percent.is_some() || facts.is_some(),
                percent,
                reset_at: poller.overview_resets_at(),
                signed_in: state,
                needs_sign_in: !state,
                key_usage_usd,
                has_budget_source,
                remaining_usd,
            })
        })
        .collect()
}

fn openrouter_card_facts(
    env: &ViewEnv<'_>,
    state: &PanelState,
    settings: &Settings,
) -> Option<(Option<f64>, bool, Option<f64>)> {
    match snapshot_view(env, state, settings, ProviderId::OpenRouter)? {
        SnapshotView::OpenRouter(Some(s)) => Some((
            Some(s.key_usage_usd),
            s.budget_source.is_some(),
            s.remaining_usd,
        )),
        _ => None,
    }
}

/// The Overview panel's view model.
pub fn overview(env: &ViewEnv<'_>, state: &PanelState) -> OverviewPanelVm {
    let settings = env.model.settings().snapshot();
    let calendar = Calendar::local(env.clock);
    let today = calendar.date_of(env.now);
    let live = today_series(
        env.model.registry(),
        &settings,
        env.sessions,
        &calendar,
        today,
    );
    let daily = env
        .overview
        .daily_input(state.offset(ProviderId::Overview), &live, today);
    build_overview(
        &OverviewInput {
            version: Some(env.version.to_owned()),
            cards: card_inputs(
                env.model,
                env.sessions,
                openrouter_card_facts(env, state, &settings),
            ),
            daily,
        },
        &PanelContext {
            now: env.now,
            calendar,
        },
    )
}

/// Builds the panel for `provider` (Overview included).
pub fn build_view(env: &ViewEnv<'_>, state: &PanelState, provider: ProviderId) -> PanelView {
    let settings = env.model.settings().snapshot();
    let ctx = PanelContext {
        now: env.now,
        calendar: Calendar::local(env.clock),
    };
    let status = status(env, provider);
    let view = snapshot_view(env, state, &settings, provider);
    match (provider, view) {
        (ProviderId::Overview, _) => PanelView::Overview(overview(env, state)),
        (ProviderId::Grok, v) => PanelView::Grok(build_grok(
            &GrokInput {
                status,
                is_signed_in: env.sessions.auth_state(provider).is_signed_in,
                snapshot: v.and_then(|v| match v {
                    SnapshotView::Grok(s) => s,
                    _ => None,
                }),
            },
            &ctx,
        )),
        (ProviderId::Grokbot, v) => PanelView::Grokbot(build_grokbot(
            &GrokbotInput {
                status,
                snapshot: v.and_then(|v| match v {
                    SnapshotView::Grokbot(s) => s,
                    _ => None,
                }),
            },
            &ctx,
        )),
        (ProviderId::Cursor, v) => PanelView::Cursor(build_cursor(
            &CursorInput {
                status,
                snapshot: v.and_then(|v| match v {
                    SnapshotView::Cursor(s) => s,
                    _ => None,
                }),
            },
            &ctx,
        )),
        (ProviderId::Claude, v) => {
            let (snapshot, code) = match v {
                Some(SnapshotView::Claude(s, c)) => (s, c),
                _ => (None, None),
            };
            PanelView::Claude(build_claude(
                &ClaudeInput {
                    status,
                    snapshot,
                    code,
                },
                &ctx,
            ))
        }
        (ProviderId::ChatGpt, v) => PanelView::ChatGpt(build_chatgpt(
            &ChatGptInput {
                status,
                snapshot: v.and_then(|v| match v {
                    SnapshotView::ChatGpt(s) => s,
                    _ => None,
                }),
            },
            &ctx,
        )),
        (ProviderId::OpenCode, v) => PanelView::OpenCode(build_opencode(
            &OpenCodeInput {
                status,
                snapshot: v.and_then(|v| match v {
                    SnapshotView::OpenCode(s) => s,
                    _ => None,
                }),
            },
            &ctx,
        )),
        (ProviderId::OpenRouter, v) => PanelView::OpenRouter(build_openrouter(
            &OpenRouterInput {
                status,
                snapshot: v.and_then(|v| match v {
                    SnapshotView::OpenRouter(s) => s,
                    _ => None,
                }),
                key_draft: state.key_draft.clone(),
                is_replacing: state.replacing_key,
                auth_error: state
                    .key_error
                    .clone()
                    .or_else(|| env.sessions.auth_state(provider).last_auth_error),
            },
            &ctx,
        )),
    }
}
