//! Keyed LRU of rendered bitmaps.

use std::collections::VecDeque;
use std::sync::Arc;

use super::render::{TrayBitmap, TrayIconRequest, render_tray_icon};

/// Entries kept, matching the macOS `NSCache.countLimit`.
pub const TRAY_CACHE_CAPACITY: usize = 40;

/// LRU cache of rendered icons keyed by [`TrayIconRequest::key`], which
/// covers every drawn input including size and taskbar theme.
#[derive(Debug, Default)]
pub struct TrayIconCache {
    /// Most recently used at the back.
    entries: VecDeque<(String, Arc<TrayBitmap>)>,
}

impl TrayIconCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of cached bitmaps.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the bitmap for `req`, rendering it only on a key miss.
    pub fn get_or_render(&mut self, req: &TrayIconRequest) -> Arc<TrayBitmap> {
        let key = req.key();
        if let Some(hit) = self
            .entries
            .iter()
            .position(|(k, _)| *k == key)
            .and_then(|pos| self.entries.remove(pos))
        {
            let bitmap = Arc::clone(&hit.1);
            self.entries.push_back(hit);
            return bitmap;
        }
        let bitmap = Arc::new(render_tray_icon(req));
        self.entries.push_back((key, Arc::clone(&bitmap)));
        if self.entries.len() > TRAY_CACHE_CAPACITY {
            self.entries.pop_front();
        }
        bitmap
    }
}
