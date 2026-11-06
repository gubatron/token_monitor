//! Panel views and the panel-event reducer, over the real composition root
//! with fakes (spec 04 section 3, spec 01 section 4).

use std::sync::Arc;

use chrono::{FixedOffset, TimeZone, Utc};
use tokenmon_app::app::{App, AppInputs};
use tokenmon_app::overview::OverviewStore;
use tokenmon_app::panel::{
    Action, PanelState, PanelView, ViewEnv, build_view, card_inputs, commit_api_key, overview,
};
use tokenmon_core::ProviderId;
use tokenmon_core::SystemClock;
use tokenmon_core::alerts::RecordingNotifier;
use tokenmon_core::settings::PanelSection;
use tokenmon_core::storage::AppPaths;
use tokenmon_platform::secrets::{APP_FALLBACK_POLICY, FakeEncryptor};
use tokenmon_platform::webauth::FakeWebAuthHost;
use tokenmon_provider_opencode::db::DataDirEnv;
use tokenmon_ui::panels::common::PanelKind;
use tokenmon_ui::panels::events::PanelEvent;
use tokenmon_ui::panels::switcher::build_switcher;

fn build(dir: &std::path::Path) -> App {
    App::build(
        AppInputs {
            paths: AppPaths::new(dir),
            encryptor: Arc::new(FakeEncryptor::default()),
            fallback: APP_FALLBACK_POLICY,
            web_host: Arc::new(FakeWebAuthHost::default()),
            notifier: Arc::new(RecordingNotifier::default()),
            clock: Arc::new(SystemClock),
            version: "0.0.0-test".to_owned(),
            home: Some(dir.to_path_buf()),
            opencode_env: DataDirEnv::default(),
        },
        tokio::runtime::Handle::current(),
    )
    .expect("builds")
}

fn tz() -> FixedOffset {
    FixedOffset::east_opt(0).expect("utc")
}

/// Every provider's panel builds from the live model. Signed out, each shows
/// its sign-in prompt (OpenRouter its key entry) rather than failing.
#[tokio::test]
async fn test_every_provider_panel_builds_signed_out() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = build(dir.path());
    let store = OverviewStore::open(dir.path());
    let env = ViewEnv {
        model: &app.model,
        sessions: &app.sessions,
        overview: &store,
        version: "1.2.3",
        now: Utc.with_ymd_and_hms(2026, 7, 10, 12, 0, 0).unwrap(),
        tz: tz(),
    };
    let state = PanelState::new(ProviderId::Overview);
    for provider in [ProviderId::Overview]
        .into_iter()
        .chain(ProviderId::USAGE_PROVIDERS)
    {
        let view = build_view(&env, &state, provider);
        match (provider, view) {
            (ProviderId::Overview, PanelView::Overview(vm)) => {
                assert_eq!(vm.version_text, "v1.2.3");
                assert_eq!(vm.daily.chart.columns.len(), 0, "no activity yet");
            }
            (ProviderId::Grok, PanelView::Grok(vm)) => assert_eq!(vm.kind, PanelKind::SignedOut),
            (ProviderId::Grokbot, PanelView::Grokbot(vm)) => {
                assert_eq!(vm.kind, PanelKind::SignedOut);
                // Grokbot signs in through Cursor.
                assert_eq!(vm.sign_in.expect("button").provider, ProviderId::Cursor);
            }
            (ProviderId::Cursor, PanelView::Cursor(vm)) => {
                assert_eq!(vm.kind, PanelKind::SignedOut);
            }
            (ProviderId::Claude, PanelView::Claude(vm)) => {
                assert_eq!(vm.panel.kind, PanelKind::SignedOut);
            }
            (ProviderId::ChatGpt, PanelView::ChatGpt(vm)) => {
                assert_eq!(vm.kind, PanelKind::SignedOut);
            }
            (ProviderId::OpenCode, PanelView::OpenCode(vm)) => {
                assert_eq!(vm.kind, PanelKind::SignedOut);
            }
            (ProviderId::OpenRouter, PanelView::OpenRouter(vm)) => {
                assert!(vm.key_entry.is_some(), "signed out shows the key field");
            }
            (p, v) => panic!("wrong view for {p}: {v:?}"),
        }
    }
}

fn fixture() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = build(dir.path());
    (dir, app)
}

fn rows(state: &PanelState, app: &App) -> Vec<tokenmon_ui::panels::switcher::RowVm> {
    build_switcher(
        &app.settings.snapshot().visible_usage_providers(),
        state.selected,
    )
}

/// Clicking a switcher cell resolves to that provider through the applied
/// grid, four cells per row; padding cells do nothing.
#[tokio::test]
async fn test_select_tab_resolves_through_the_switcher_grid() {
    let (_dir, app) = fixture();
    let mut state = PanelState::new(ProviderId::Overview);
    let grid = rows(&state, &app);
    assert_eq!(
        state.handle(&PanelEvent::SelectTab { row: 0, col: 0 }, &grid, &[]),
        vec![Action::Show(ProviderId::Overview)]
    );
    let visible = app.settings.snapshot().visible_usage_providers();
    // Row 1 starts with the fourth provider (Overview takes slot 0).
    assert_eq!(
        state.handle(&PanelEvent::SelectTab { row: 1, col: 0 }, &grid, &[]),
        vec![Action::Show(visible[3])]
    );
    assert_eq!(
        state.handle(&PanelEvent::SelectTab { row: 9, col: 9 }, &grid, &[]),
        Vec::<Action>::new()
    );
}

/// Week arrows page per provider: earlier goes down without bound, later
/// stops at the current period.
#[tokio::test]
async fn test_period_paging_is_per_provider_and_never_future() {
    let (_dir, app) = fixture();
    let mut state = PanelState::new(ProviderId::Claude);
    let grid = rows(&state, &app);
    let prev = PanelEvent::PreviousPeriod(ProviderId::Claude);
    let next = PanelEvent::NextPeriod(ProviderId::Claude);
    assert_eq!(state.handle(&prev, &grid, &[]), vec![Action::Redraw]);
    state.handle(&prev, &grid, &[]);
    assert_eq!(state.offset(ProviderId::Claude), -2);
    assert_eq!(state.offset(ProviderId::Grok), 0);
    for _ in 0..5 {
        state.handle(&next, &grid, &[]);
    }
    assert_eq!(state.offset(ProviderId::Claude), 0);
}

/// Sign-in buttons, models toggles, View all and the key flow reduce to the
/// documented actions.
#[tokio::test]
async fn test_events_reduce_to_actions() {
    let (_dir, app) = fixture();
    let mut state = PanelState::new(ProviderId::OpenRouter);
    let g = rows(&state, &app);
    assert_eq!(
        state.handle(&PanelEvent::SignIn(ProviderId::Cursor), &g, &[]),
        vec![Action::SignIn(ProviderId::Cursor)]
    );
    assert_eq!(
        state.handle(&PanelEvent::SignOut(ProviderId::Grok), &g, &[]),
        vec![Action::SignOut(ProviderId::Grok)]
    );
    assert_eq!(
        state.handle(&PanelEvent::OpenSettings, &g, &[]),
        vec![Action::OpenSettings]
    );
    assert_eq!(
        state.handle(
            &PanelEvent::ToggleModelsSection(ProviderId::Claude),
            &g,
            &[]
        ),
        vec![Action::ToggleSection(PanelSection::ClaudeModels)]
    );
    assert_eq!(
        state.handle(&PanelEvent::ToggleModelsSection(ProviderId::Grok), &g, &[]),
        Vec::<Action>::new()
    );
    state.handle(
        &PanelEvent::ToggleShowAllModels(ProviderId::OpenRouter),
        &g,
        &[],
    );
    assert!(state.shows_all(ProviderId::OpenRouter));
    state.handle(
        &PanelEvent::ToggleShowAllModels(ProviderId::OpenRouter),
        &g,
        &[],
    );
    assert!(!state.shows_all(ProviderId::OpenRouter));
    // Typing stores the draft without a redraw (the field keeps its cursor).
    assert_eq!(
        state.handle(&PanelEvent::ApiKeyEdited("sk-or-x".into()), &g, &[]),
        Vec::<Action>::new()
    );
    assert_eq!(state.key_draft, "sk-or-x");
    assert_eq!(
        state.handle(&PanelEvent::ReplaceApiKey, &g, &[]),
        vec![Action::Redraw]
    );
    assert!(state.replacing_key);
}

/// Overview card buttons resolve against the applied cards: a sign-in button
/// opens the sign-in window or, for OpenRouter, selects its tab.
#[tokio::test]
async fn test_overview_card_actions_resolve_against_applied_cards() {
    let (dir, app) = fixture();
    let store = OverviewStore::open(dir.path());
    let env = ViewEnv {
        model: &app.model,
        sessions: &app.sessions,
        overview: &store,
        version: "1",
        now: Utc::now(),
        tz: tz(),
    };
    let mut state = PanelState::new(ProviderId::Overview);
    let vm = overview(&env, &state);
    let g = rows(&state, &app);
    let first = vm.cards[0].provider;
    assert_eq!(
        state.handle(&PanelEvent::OverviewSignIn(0), &g, &vm.cards),
        vec![Action::SignIn(first)]
    );
    let or = vm
        .cards
        .iter()
        .position(|c| c.provider == ProviderId::OpenRouter)
        .expect("openrouter card");
    assert_eq!(
        state.handle(&PanelEvent::OverviewSignIn(or), &g, &vm.cards),
        vec![Action::Show(ProviderId::OpenRouter)]
    );
    assert_eq!(
        state.handle(&PanelEvent::OverviewSignIn(99), &g, &vm.cards),
        Vec::<Action>::new()
    );
    assert_eq!(
        card_inputs(&app.model, &app.sessions, None).len(),
        vm.cards.len()
    );
}

/// A malformed OpenRouter key is refused with the session's message and the
/// draft is kept; a valid key signs the API-key session in and leaves the
/// replace flow.
#[tokio::test]
async fn test_api_key_commit_reports_errors_and_success() {
    let (_dir, app) = fixture();
    let mut state = PanelState::new(ProviderId::OpenRouter);
    state.key_draft = "nope".into();
    state.replacing_key = true;
    assert!(!commit_api_key(&mut state, &app.sessions, "nope"));
    assert!(state.key_error.is_some());
    assert_eq!(state.key_draft, "nope");
    assert!(!app.sessions.signed_in(ProviderId::OpenRouter));

    assert!(commit_api_key(&mut state, &app.sessions, "sk-or-v1-abc"));
    assert!(state.key_error.is_none() && state.key_draft.is_empty() && !state.replacing_key);
    assert!(app.sessions.signed_in(ProviderId::OpenRouter));
}
