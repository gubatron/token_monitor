//! Tooltip text (`NOTIFYICONDATA.szTip`).

use tokenmon_core::ProviderId;

use super::content::TrayContent;

/// `szTip` holds 128 UTF-16 units including the terminating NUL.
pub const TOOLTIP_MAX_UTF16: usize = 127;

/// One tooltip line item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TooltipEntry {
    /// The provider.
    pub provider: ProviderId,
    /// Used percent, `None` for no data.
    pub used: Option<f64>,
    /// Signed out: shows just the provider name (Grok on macOS).
    pub signed_out: bool,
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn entry_text(e: &TooltipEntry) -> String {
    let name = e.provider.display_name();
    if e.signed_out {
        return name.to_owned();
    }
    match e.used {
        Some(v) => format!("{name} {}%", v.clamp(0.0, 100.0).round() as i32),
        None => format!("{name} \u{2014}"),
    }
}

/// Builds `Grok 41% · Cursor 12% · OpenCode — · Claude 63%`.
///
/// If it exceeds [`TOOLTIP_MAX_UTF16`] UTF-16 units, whole trailing entries
/// are dropped and `" …"` is appended, so no entry is cut mid-way.
///
/// ```
/// use tokenmon_core::ProviderId::*;
/// use tokenmon_ui::tray_icon::{build_tooltip, TooltipEntry};
/// let e = |provider, used| TooltipEntry { provider, used, signed_out: false };
/// assert_eq!(build_tooltip(&[e(Grok, Some(41.2)), e(Cursor, None)]), "Grok 41% · Cursor —");
/// ```
pub fn build_tooltip(entries: &[TooltipEntry]) -> String {
    let parts: Vec<String> = entries.iter().map(entry_text).collect();
    let full = parts.join(" \u{b7} ");
    if utf16_len(&full) <= TOOLTIP_MAX_UTF16 {
        return full;
    }
    (0..parts.len())
        .rev()
        .map(|n| format!("{} \u{2026}", parts[..n].join(" \u{b7} ")))
        .find(|s| utf16_len(s) <= TOOLTIP_MAX_UTF16)
        .unwrap_or_else(|| "\u{2026}".to_owned())
}

/// The tooltip for what the icon actually draws: one entry per icon segment
/// (spec 04 section 2.7), so the text and the bars can never disagree.
///
/// Only Grok signed out shows a bare name (the Mac rule); every other
/// segment without a value reads `Name \u{2014}`, and a segment with a value
/// always shows its percent, even when the session needs sign-in.
///
/// ```
/// use tokenmon_core::ProviderId::*;
/// use tokenmon_ui::tray_icon::{TrayContent, tooltip_for_content};
/// let c = TrayContent::Selected { provider: OpenCode, used: Some(12.0) };
/// assert_eq!(tooltip_for_content(&c, true), "OpenCode 12%");
/// ```
pub fn tooltip_for_content(content: &TrayContent, grok_signed_in: bool) -> String {
    let entry = |provider: ProviderId, used: Option<f64>| TooltipEntry {
        provider,
        used,
        signed_out: provider == ProviderId::Grok && !grok_signed_in,
    };
    let entries: Vec<TooltipEntry> = match content {
        TrayContent::Empty => return "TokenMon".to_owned(),
        TrayContent::Selected { provider, used } => vec![entry(*provider, *used)],
        TrayContent::Bars(bars) => bars.iter().map(|b| entry(b.provider, b.used)).collect(),
    };
    build_tooltip(&entries)
}

/// Encodes `text` into a NUL-terminated `szTip` buffer, truncating on a
/// char boundary (never splitting a surrogate pair).
pub fn encode_tip_utf16(text: &str) -> [u16; 128] {
    let mut buf = [0u16; 128];
    let mut n = 0;
    for ch in text.chars() {
        let mut tmp = [0u16; 2];
        let units = ch.encode_utf16(&mut tmp);
        if n + units.len() > TOOLTIP_MAX_UTF16 {
            break;
        }
        buf[n..n + units.len()].copy_from_slice(units);
        n += units.len();
    }
    buf
}
