//! Rasterizes the tray icon with tiny-skia.
#![allow(clippy::chunks_exact_to_as_chunks)] // as_chunks needs a newer MSRV than rust-version guarantees

use tiny_skia::{Color, Paint, Pixmap};
use tokenmon_core::ProviderId;

use super::content::{BarSegment, TrayContent};
use super::glyphs::{digits_height, digits_width, draw_digits, draw_logo, fill_rect, fill_rrect};
use super::theme::TaskbarTheme;

/// Alpha of the bar track relative to the chrome color (spec 04 section 2.2).
const TRACK_ALPHA: f32 = 0.22;
/// Smallest visible fill, in pixels, even at 0 %.
const MIN_FILL_PX: f32 = 2.0;

/// Square icon sizes Windows asks for (`SM_CXSMICON` at 100/125/150/200/250 %).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrayIconSize {
    /// 100 % DPI.
    Px16,
    /// 125 % DPI.
    Px20,
    /// 150 % DPI.
    Px24,
    /// 200 % DPI.
    Px32,
    /// 250 % DPI.
    Px40,
}

impl TrayIconSize {
    /// Edge length in pixels.
    pub const fn pixels(self) -> u32 {
        match self {
            Self::Px16 => 16,
            Self::Px20 => 20,
            Self::Px24 => 24,
            Self::Px32 => 32,
            Self::Px40 => 40,
        }
    }

    /// The supported size closest to `SM_CXSMICON` (ties go larger).
    ///
    /// ```
    /// use tokenmon_ui::tray_icon::TrayIconSize;
    /// assert_eq!(TrayIconSize::nearest(16), TrayIconSize::Px16);
    /// assert_eq!(TrayIconSize::nearest(22), TrayIconSize::Px24);
    /// assert_eq!(TrayIconSize::nearest(999), TrayIconSize::Px40);
    /// ```
    pub fn nearest(px: u32) -> Self {
        [Self::Px16, Self::Px20, Self::Px24, Self::Px32, Self::Px40]
            .into_iter()
            .min_by_key(|s| (s.pixels().abs_diff(px), u32::MAX - s.pixels()))
            .unwrap_or(Self::Px16)
    }
}

/// Everything that determines the bitmap.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayIconRequest {
    /// What to draw.
    pub content: TrayContent,
    /// Icon edge length.
    pub size: TrayIconSize,
    /// Taskbar theme (chrome color).
    pub theme: TaskbarTheme,
}

impl TrayIconRequest {
    /// Cache key covering every drawn input: content (percentages rounded as
    /// drawn), pixel size and theme.
    pub fn key(&self) -> String {
        format!(
            "tray-{}-{}-{}",
            self.content.key(),
            self.size.pixels(),
            self.theme.as_str()
        )
    }
}

/// A rendered icon, straight (non-premultiplied) RGBA, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayBitmap {
    /// Edge length in pixels.
    pub size: u32,
    /// `size * size * 4` bytes, RGBA, straight alpha.
    pub rgba: Vec<u8>,
}

impl TrayBitmap {
    /// Pixels as BGRA with premultiplied alpha, the layout of a 32-bpp
    /// `HBITMAP` color plane for `CreateIconIndirect`.
    pub fn to_bgra_premultiplied(&self) -> Vec<u8> {
        self.rgba
            .chunks_exact(4)
            .flat_map(|p| {
                let a = u16::from(p[3]);
                let m = |c: u8| ((u16::from(c) * a + 127) / 255) as u8;
                [m(p[2]), m(p[1]), m(p[0]), p[3]]
            })
            .collect()
    }

    /// PNG encoding (used by snapshot tests).
    pub fn to_png(&self) -> Vec<u8> {
        Pixmap::from_vec(
            self.premultiplied(),
            tiny_skia::IntSize::from_wh(self.size, self.size).expect("non-zero size"),
        )
        .and_then(|p| p.encode_png().ok())
        .unwrap_or_default()
    }

    fn premultiplied(&self) -> Vec<u8> {
        self.rgba
            .chunks_exact(4)
            .flat_map(|p| {
                let a = u16::from(p[3]);
                let m = |c: u8| ((u16::from(c) * a + 127) / 255) as u8;
                [m(p[0]), m(p[1]), m(p[2]), p[3]]
            })
            .collect()
    }
}

fn paint_rgba(rgb: [u8; 3], alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(
        rgb[0],
        rgb[1],
        rgb[2],
        (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    ));
    p.anti_alias = true;
    p
}

fn accent_rgb(p: ProviderId) -> [u8; 3] {
    let (r, g, b) = p.accent().to_rgb8();
    [r, g, b]
}

/// Fill length for `used` percent of `len` pixels, never under the sliver.
fn fill_len(len: f32, used: Option<f64>) -> f32 {
    let frac = used.map_or(0.0, |u| u.clamp(0.0, 100.0) as f32 / 100.0);
    (len * frac).max(MIN_FILL_PX).min(len)
}

/// Renders the icon. Total: any request yields a bitmap.
///
/// ```
/// use tokenmon_ui::tray_icon::*;
/// let bmp = render_tray_icon(&TrayIconRequest {
///     content: TrayContent::Empty, size: TrayIconSize::Px16, theme: TaskbarTheme::Dark,
/// });
/// assert_eq!(bmp.rgba.len(), 16 * 16 * 4);
/// ```
pub fn render_tray_icon(req: &TrayIconRequest) -> TrayBitmap {
    let s = req.size.pixels();
    if req.content == TrayContent::Mark
        && let Some(rgba) = super::mark::mark_rgba(s)
    {
        return TrayBitmap { size: s, rgba };
    }
    let mut pm = Pixmap::new(s, s).expect("tray icon size is non-zero");
    let chrome = req.theme.chrome_rgb();
    match &req.content {
        TrayContent::Empty => draw_empty(&mut pm, s, chrome),
        // Only reached when the icon file lacks this size: a plain tile.
        TrayContent::Mark => draw_logo(
            &mut pm,
            ProviderId::Overview,
            0.0,
            0.0,
            s as f32,
            &paint_rgba(chrome, 1.0),
        ),
        TrayContent::Selected { provider, used } => {
            draw_selected(&mut pm, s, *provider, *used, chrome);
        }
        TrayContent::Bars(bars) => draw_bars(&mut pm, s, bars, chrome),
    }
    let rgba = pm
        .pixels()
        .iter()
        .flat_map(|px| {
            let c = px.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    TrayBitmap { size: s, rgba }
}

/// "—": a centered horizontal stroke.
fn draw_empty(pm: &mut Pixmap, s: u32, chrome: [u8; 3]) {
    let t = (s / 8).max(2) as f32;
    let w = s as f32 * 0.6;
    let s = s as f32;
    let paint = paint_rgba(chrome, 1.0);
    fill_rrect(pm, (s - w) / 2.0, (s - t) / 2.0, w, t, t / 2.0, &paint);
}

fn draw_selected(
    pm: &mut Pixmap,
    s: u32,
    provider: ProviderId,
    used: Option<f64>,
    chrome: [u8; 3],
) {
    let margin = (s / 16).max(1);
    let bar_h = (s / 8).max(2);
    let bar_w = (s - 2 * margin) as f32;
    let bar_y = (s - margin - bar_h) as f32;
    let (mf, bh) = (margin as f32, bar_h as f32);
    fill_rrect(
        pm,
        mf,
        bar_y,
        bar_w,
        bh,
        bh / 2.0,
        &paint_rgba(chrome, TRACK_ALPHA),
    );
    let fill = fill_len(bar_w, used);
    fill_rrect(
        pm,
        mf,
        bar_y,
        fill,
        bh,
        bh / 2.0,
        &paint_rgba(accent_rgb(provider), 1.0),
    );

    let region_top = margin;
    let region_h = (bar_y as u32)
        .saturating_sub(margin + region_top.max(1))
        .max(1);
    let logo_paint = paint_rgba(chrome, 1.0);
    let digits = digits_text(used).filter(|_| s >= 32);
    let scale = (s / 16).max(1);
    let logo_side = if digits.is_some() {
        (digits_height(scale) + scale).min(region_h)
    } else {
        region_h.min(s - 2 * margin)
    };
    let digits_w = digits
        .as_deref()
        .map_or(0, |d| digits_width(d.len(), scale));
    let gap = scale;
    let group_w = if digits_w > 0 {
        logo_side + gap + digits_w
    } else {
        logo_side
    };
    if digits_w > 0 && group_w > s - 2 * margin {
        // 3-digit values do not fit beside the logo: logo only.
        let side = region_h.min(s - 2 * margin);
        let x = (s - side) as f32 / 2.0;
        draw_logo(pm, provider, x, region_top as f32, side as f32, &logo_paint);
        return;
    }
    let x0 = (s - group_w) as f32 / 2.0;
    let y_logo = region_top as f32 + (region_h - logo_side) as f32 / 2.0;
    draw_logo(pm, provider, x0, y_logo, logo_side as f32, &logo_paint);
    if let Some(d) = digits {
        let ty = region_top + (region_h - digits_height(scale)) / 2;
        draw_digits(
            pm,
            &d,
            (x0 as u32) + logo_side + gap,
            ty,
            scale,
            &logo_paint,
        );
    }
}

/// `NN` text for the digit overlay; `None` (no data) shows no digits.
fn digits_text(used: Option<f64>) -> Option<String> {
    used.map(|u| format!("{}", u.clamp(0.0, 100.0).round() as i32))
}

fn draw_bars(pm: &mut Pixmap, s: u32, bars: &[BarSegment], chrome: [u8; 3]) {
    let n = bars.len().max(1) as u32;
    let margin = (s / 16).max(1);
    let gap = if s >= 24 { 2 } else { 1 };
    let avail = s - 2 * margin;
    let bar_w = ((avail - gap * (n - 1)) / n).clamp(2, (s / 4).max(3));
    let group_w = bar_w * n + gap * (n - 1);
    let x0 = (s - group_w) / 2;
    let h = (s - 2 * margin) as f32;
    let track = paint_rgba(chrome, TRACK_ALPHA);
    for (i, b) in bars.iter().enumerate() {
        let x = (x0 + i as u32 * (bar_w + gap)) as f32;
        fill_rect(pm, x, margin as f32, bar_w as f32, h, &track);
        let fh = fill_len(h, b.used);
        let accent = paint_rgba(accent_rgb(b.provider), 1.0);
        fill_rect(pm, x, margin as f32 + h - fh, bar_w as f32, fh, &accent);
    }
}
