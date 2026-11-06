//! Logoff / shutdown handling (#71): which messages quit the app.

use tokenmon_app::session::{SessionMessage, WM_ENDSESSION, WM_QUERYENDSESSION, classify};

/// The query is answered "yes, end" and never quits by itself.
#[test]
fn test_query_allows_the_session_to_end_without_quitting() {
    let m = classify(WM_QUERYENDSESSION, 0).expect("query");
    assert_eq!(m, SessionMessage::Query);
    assert_eq!(m.reply(), 1);
    assert!(!m.must_quit());
}

/// A real end runs the quit path.
#[test]
fn test_end_session_quits() {
    let m = classify(WM_ENDSESSION, 1).expect("end");
    assert!(m.must_quit());
    assert_eq!(m.reply(), 0);
}

/// A vetoed end must not close the app.
#[test]
fn test_cancelled_end_session_does_not_quit() {
    assert!(!classify(WM_ENDSESSION, 0).expect("end").must_quit());
}

/// Unrelated messages fall through to the default handler.
#[test]
fn test_other_messages_are_ignored() {
    for msg in [0, 0x000F, 0x0010, 0x0400] {
        assert_eq!(classify(msg, 1), None);
    }
}
