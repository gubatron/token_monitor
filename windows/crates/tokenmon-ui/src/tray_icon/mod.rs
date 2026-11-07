//! Tray bitmap renderer (tiny-skia), tooltip text and menu model.
//!
//! Owner: ws-tray. Source: spec 04 section 2.7 (Windows tray mapping).
//!
//! Everything here is a pure function of its inputs; the `tray-icon` crate
//! wiring (and the registry read behind [`TaskbarThemeSource`]) belongs to
//! ws-app. The pipeline is:
//!
//! 1. [`TrayContent::from_inputs`] decides what to draw.
//! 2. [`TrayIconRequest`] adds the pixel size and taskbar theme.
//! 3. [`TrayIconCache`] returns the bitmap, rendering only on a key miss.
//! 4. [`TrayBitmap::to_bgra_premultiplied`] feeds `CreateIconIndirect`.

mod cache;
mod content;
mod glyphs;
mod mark;
mod menu;
mod render;
mod theme;
mod tooltip;

pub use cache::{TRAY_CACHE_CAPACITY, TrayIconCache};
pub use content::{BarSegment, MAX_BARS, TrayContent, TrayInputs};
pub use mark::{decode_ico_frame, mark_rgba};
pub use menu::{MenuAction, MenuEntry, MenuInputs, build_menu};
pub use render::{TrayBitmap, TrayIconRequest, TrayIconSize, render_tray_icon};
pub use theme::{FixedThemeSource, TaskbarTheme, TaskbarThemeSource};
pub use tooltip::{
    TOOLTIP_MAX_UTF16, TooltipEntry, build_tooltip, encode_tip_utf16, tooltip_for_content,
};
