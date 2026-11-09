//! Overview panel input built from the live model.

use std::sync::Arc;

use tokenmon_app::app::{App, AppInputs};
use tokenmon_core::SystemClock;
use tokenmon_core::alerts::RecordingNotifier;
use tokenmon_core::storage::AppPaths;
use tokenmon_platform::secrets::FakeEncryptor;
use tokenmon_platform::webauth::FakeWebAuthHost;
use tokenmon_provider_opencode::db::DataDirEnv;

fn build(dir: &std::path::Path) -> App {
    App::build(
        AppInputs {
            paths: AppPaths::new(dir),
            encryptor: Arc::new(FakeEncryptor::default()),
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

/// One signed-out card per visible provider, in the user's order, building
/// into a view model with the version text.
#[tokio::test]
async fn test_overview_cards_follow_visible_providers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = build(dir.path());
    let cards = tokenmon_app::panel::card_inputs(&app.model, &app.sessions, None);
    let visible = app.settings.snapshot().visible_usage_providers();
    assert_eq!(
        cards.iter().map(|c| c.provider).collect::<Vec<_>>(),
        visible
    );
    assert!(cards.iter().all(|c| !c.signed_in && c.needs_sign_in));
    let store = tokenmon_app::overview::OverviewStore::open(dir.path());
    let env = tokenmon_app::panel::ViewEnv {
        model: &app.model,
        sessions: &app.sessions,
        overview: &store,
        version: "1.2.3",
        now: chrono::Utc::now(),
        clock: &tokenmon_core::ManualClock::new(chrono::Utc::now()),
    };
    let vm = tokenmon_app::panel::overview(
        &env,
        &tokenmon_app::panel::PanelState::new(tokenmon_core::ProviderId::Overview),
    );
    assert_eq!(vm.cards.len(), visible.len());
    assert_eq!(vm.version_text, "v1.2.3");
}

/// #55 audit: an OpenRouter key without a credit limit has a snapshot but no
/// percent; the card still counts as having a snapshot (it shows the key's
/// spend, not "No OpenRouter data yet"), and every other provider's card keeps
/// its "no snapshot" state until its poller publishes one.
#[tokio::test]
async fn test_openrouter_snapshot_without_percent_still_has_a_snapshot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = build(dir.path());
    let facts = Some((Some(4.5), false, None));
    let cards = tokenmon_app::panel::card_inputs(&app.model, &app.sessions, facts);
    let by = |p| {
        cards
            .iter()
            .find(|c| c.provider == p)
            .map(|c| c.has_snapshot)
    };
    assert_eq!(by(tokenmon_core::ProviderId::OpenRouter), Some(true));
    assert_eq!(by(tokenmon_core::ProviderId::Grok), Some(false));
    let none = tokenmon_app::panel::card_inputs(&app.model, &app.sessions, None);
    assert!(none.iter().all(|c| !c.has_snapshot));
}
