//! Panel view models: plain Rust, display-ready data for the Slint panels in
//! `ui/panels/`. Every string, number format and state of spec 02/03 panel
//! sections and spec 04 section 3 is decided here and unit-tested; the
//! `.slint` files only lay out what these models contain.

pub mod claude_chatgpt;
pub mod common;
pub mod format;
pub mod grok;
pub mod grokbot_cursor;
pub mod opencode_openrouter;
pub mod overview;
pub mod switcher;
