//! Overview panel input built from the live model.

use std::sync::Arc;

use tokenmon_app::app::{App, AppInputs};
use tokenmon_core::SystemClock;
use tokenmon_core::alerts::RecordingNotifier;
use tokenmon_core::storage::AppPaths;
use tokenmon_platform::secrets::{APP_FALLBACK_POLICY, FakeEncryptor};
use tokenmon_platform::webauth::FakeWebAuthHost;
use tokenmon_provider_opencode::db::DataDirEnv;

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
