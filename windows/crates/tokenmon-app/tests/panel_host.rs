//! One panel window for every tab (issue #81): switching tabs never creates a
//! second window, and the selected tab persists in the settings.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use tokenmon_app::app::{App, AppInputs};
use tokenmon_app::panel::{Action, PanelState};
use tokenmon_app::panel_host::{SinglePanel, select_tab};
use tokenmon_core::ProviderId;
use tokenmon_core::SystemClock;
use tokenmon_core::alerts::RecordingNotifier;
use tokenmon_core::storage::AppPaths;
use tokenmon_platform::secrets::{APP_FALLBACK_POLICY, FakeEncryptor};
use tokenmon_platform::webauth::FakeWebAuthHost;
use tokenmon_provider_opencode::db::DataDirEnv;
use tokenmon_ui::panels::events::PanelEvent;
use tokenmon_ui::panels::switcher::build_switcher;

/// A stand-in for the Slint window.
struct FakeWindow(usize);

fn counting_panel() -> (SinglePanel<FakeWindow>, Rc<Cell<usize>>) {
    let built = Rc::new(Cell::new(0));
    let counter = Rc::clone(&built);
    let panel = SinglePanel::new(Box::new(move || {
        counter.set(counter.get() + 1);
        Ok(Rc::new(FakeWindow(counter.get())))
    }));
    (panel, built)
}

/// Clicking every switcher tab in turn asks the factory for the window each
/// time but builds exactly one, and always hands back that same window.
#[test]
fn test_switching_every_tab_never_creates_a_second_window() {
    let (panel, built) = counting_panel();
    let mut state = PanelState::new(ProviderId::Claude);
    assert!(
        panel.existing().is_none(),
        "nothing is built before first use"
    );
    let first = panel.get_or_create().expect("creates");
    for provider in ProviderId::USAGE_PROVIDERS {
        let rows = build_switcher(&ProviderId::USAGE_PROVIDERS, state.selected);
        let (row, col) = (0..rows.len())
            .flat_map(|r| (0..4).map(move |c| (r, c)))
            .find(|&(r, c)| {
                rows[r].cells[c]
                    .tab
                    .as_ref()
                    .is_some_and(|t| t.provider == provider)
            })
            .expect("every tab has a cell");
        let actions = state.handle(&PanelEvent::SelectTab { row, col }, &rows, &[]);
        assert_eq!(actions, vec![Action::Show(provider)]);
        select_tab(&mut state, provider, &|_| {});
        let window = panel.get_or_create().expect("same window");
        assert!(Rc::ptr_eq(&first, &window));
    }
    assert_eq!(built.get(), 1);
    assert_eq!(panel.creations(), 1);
    assert_eq!(first.0, 1);
}

/// A failed creation is reported and retried, and only a success counts.
#[test]
fn test_failed_window_creation_is_retried_and_not_counted() {
    let attempts = Rc::new(Cell::new(0));
    let seen = Rc::clone(&attempts);
    let panel: SinglePanel<FakeWindow> = SinglePanel::new(Box::new(move || {
        seen.set(seen.get() + 1);
        if seen.get() == 1 {
            Err("no display".to_owned())
        } else {
            Ok(Rc::new(FakeWindow(seen.get())))
        }
    }));
    assert_eq!(panel.get_or_create().err().as_deref(), Some("no display"));
    assert_eq!(panel.creations(), 0);
    panel.get_or_create().expect("second try builds");
    panel.get_or_create().expect("then reuses");
    assert_eq!((attempts.get(), panel.creations()), (2, 1));
}

/// `select_tab` reports a change only for a different tab and always hands
/// the provider to the persistence hook.
#[test]
fn test_select_tab_reports_change_and_persists_every_time() {
    let saved = RefCell::new(Vec::new());
    let mut state = PanelState::new(ProviderId::Grok);
    let persist = |p| saved.borrow_mut().push(p);
    assert!(!select_tab(&mut state, ProviderId::Grok, &persist));
    assert!(select_tab(&mut state, ProviderId::Cursor, &persist));
    assert_eq!(state.selected, ProviderId::Cursor);
    assert_eq!(*saved.borrow(), vec![ProviderId::Grok, ProviderId::Cursor]);
}

/// The selected tab lands in `AppSettings` (`selectedProvider`), so the next
/// launch opens on it.
#[tokio::test]
async fn test_selected_tab_persists_in_app_settings() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = App::build(
        AppInputs {
            paths: AppPaths::new(dir.path()),
            encryptor: Arc::new(FakeEncryptor::default()),
            fallback: APP_FALLBACK_POLICY,
            web_host: Arc::new(FakeWebAuthHost::default()),
            notifier: Arc::new(RecordingNotifier::default()),
            clock: Arc::new(SystemClock),
            version: "0.0.0-test".to_owned(),
            home: Some(dir.path().to_path_buf()),
            opencode_env: DataDirEnv::default(),
        },
        tokio::runtime::Handle::current(),
    )
    .expect("builds");
    let mut state = PanelState::new(app.settings.snapshot().selected_provider);
    select_tab(&mut state, ProviderId::Cursor, &|p| {
        app.settings.set_selected_provider(p);
    });
    assert_eq!(
        app.settings.snapshot().selected_provider,
        ProviderId::Cursor
    );
}
