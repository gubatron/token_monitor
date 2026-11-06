//! Keeps the Grok history on disk: a debounced flush while the app runs and a
//! final flush on quit (the Mac app's 400 ms dirty flag and its
//! `applicationWillTerminate` flush).
//!
//! `GrokHistoryStore::append` only marks the store dirty; nothing writes until
//! somebody calls `flush`. The store lives in a type-erased poller, so this
//! module watches the dirty flag through [`HistoryAccess`] instead of
//! hooking the poller.

use std::sync::Arc;
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::model::AppModel;
use crate::sources::HistoryAccess;

/// How often the dirty flag is checked: a change reaches the disk within
/// twice this after the append. The value is the Grok crate's debounce.
pub const FLUSH_DEBOUNCE: Duration = tokenmon_provider_grok::history::FLUSH_DEBOUNCE;

/// Writes `history` when it changed. Returns whether a flush was attempted.
pub fn flush_if_dirty(history: &dyn HistoryAccess) -> bool {
    if history.is_dirty() {
        history.flush();
        true
    } else {
        false
    }
}

/// Spawns the debounced flusher; it runs until aborted (or the runtime ends).
pub fn spawn_history_flusher(handle: &Handle, history: Arc<dyn HistoryAccess>) -> JoinHandle<()> {
    handle.spawn(async move {
        let mut tick = tokio::time::interval(FLUSH_DEBOUNCE);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            flush_if_dirty(history.as_ref());
        }
    })
}

/// Registers the final flush: it runs once, with the other quit hooks, on
/// every quit path.
pub fn flush_on_quit(model: &AppModel, history: Arc<dyn HistoryAccess>) {
    model.on_quit(move || history.flush());
}
