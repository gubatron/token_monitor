//! Vector silhouettes standing in for the provider logos, plus a 3x5 digit
//! font. The brand SVGs (owned by ws-ui-kit) are too detailed for 16 px; these
//! are deliberately simple template masks tinted with the chrome color.

use tiny_skia::{FillRule, LineCap, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};
use tokenmon_core::ProviderId;

/// 3x5 glyph rows, MSB of the low 3 bits is the left pixel. Index 10 is `-`.
const DIGITS: [[u8; 5]; 11] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
    [0b000, 0b000, 0b111, 0b000, 0b000],
];

/// Pixel width of `n` digits at `scale` (3 px per glyph, 1 px gap, scaled).
pub fn digits_width(n: usize, scale: u32) -> u32 {
    if n == 0 {
        0
    } else {
        n as u32 * 3 * scale + (n as u32 - 1) * scale
    }
}

/// Pixel height of the digit font at `scale`.
pub fn digits_height(scale: u32) -> u32 {
    5 * scale
}

/// Draws decimal `text` (digits only) with its top-left at `(x, y)`.
pub fn draw_digits(pm: &mut Pixmap, text: &str, x: u32, y: u32, scale: u32, paint: &Paint<'_>) {
    let mut cx = x;
    for ch in text.chars() {
        let Some(d) = ch.to_digit(10) else { continue };
        for (row, bits) in DIGITS[d as usize].iter().enumerate() {
            for col in 0..3u32 {
                if bits >> (2 - col) & 1 == 1 {
                    fill_rect(
                        pm,
                        (cx + col * scale) as f32,
                        (y + row as u32 * scale) as f32,
                        scale as f32,
                        scale as f32,
                        paint,
                    );
                }
            }
        }
        cx += 4 * scale;
    }
}

/// Fills an axis-aligned rectangle (no-op when degenerate).
pub fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, paint: &Paint<'_>) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, paint, Transform::identity(), None);
    }
}

/// Fills a rounded rectangle with corner radius clamped to half the short side.
#[allow(clippy::many_single_char_names)]
pub fn fill_rrect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, paint: &Paint<'_>) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    if let Some(p) = pb.finish() {
        pm.fill_path(&p, paint, FillRule::Winding, Transform::identity(), None);
    }
}

fn stroke(pm: &mut Pixmap, path: &Path, width: f32, paint: &Paint<'_>, t: Transform) {
    let s = Stroke {
        width,
        line_cap: LineCap::Round,
        ..Stroke::default()
    };
    pm.stroke_path(path, paint, &s, t, None);
}

fn poly(points: &[(f32, f32)], closed: bool) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let (first, rest) = points.split_first()?;
    pb.move_to(first.0, first.1);
    for p in rest {
        pb.line_to(p.0, p.1);
    }
    if closed {
        pb.close();
    }
    pb.finish()
}

/// Draws the silhouette for `provider` (Overview = the TokenMon mark) in a
/// `side` x `side` box at `(x, y)`.
pub fn draw_logo(
    pm: &mut Pixmap,
    provider: ProviderId,
    x: f32,
    y: f32,
    side: f32,
    paint: &Paint<'_>,
) {
    let t = Transform::from_translate(x, y).pre_scale(side, side);
    let w = 0.16; // stroke width in unit coordinates
    match provider {
        ProviderId::Grok => {
            if let Some(c) = PathBuilder::from_circle(0.5, 0.5, 0.38) {
                stroke(pm, &c, w, paint, t);
            }
            if let Some(l) = poly(&[(0.18, 0.88), (0.82, 0.12)], false) {
                stroke(pm, &l, w, paint, t);
            }
        }
        ProviderId::Cursor => {
            if let Some(p) = poly(
                &[
                    (0.5, 0.04),
                    (0.92, 0.28),
                    (0.92, 0.72),
                    (0.5, 0.96),
                    (0.08, 0.72),
                    (0.08, 0.28),
                ],
                true,
            ) {
                stroke(pm, &p, w, paint, t);
            }
        }
        ProviderId::OpenCode => {
            for pts in [
                [(0.34, 0.12), (0.12, 0.12), (0.12, 0.88), (0.34, 0.88)],
                [(0.66, 0.12), (0.88, 0.12), (0.88, 0.88), (0.66, 0.88)],
            ] {
                if let Some(p) = poly(&pts, false) {
                    stroke(pm, &p, w, paint, t);
                }
            }
        }
        ProviderId::Claude => {
            for (a, b) in [
                ((0.5, 0.06), (0.5, 0.94)),
                ((0.1, 0.28), (0.9, 0.72)),
                ((0.1, 0.72), (0.9, 0.28)),
            ] {
                if let Some(l) = poly(&[a, b], false) {
                    stroke(pm, &l, w, paint, t);
                }
            }
        }
        ProviderId::ChatGpt => {
            if let Some(p) = poly(
                &[
                    (0.5, 0.04),
                    (0.9, 0.27),
                    (0.9, 0.73),
                    (0.5, 0.96),
                    (0.1, 0.73),
                    (0.1, 0.27),
                ],
                true,
            ) {
                stroke(pm, &p, w, paint, t);
            }
            if let Some(c) = PathBuilder::from_circle(0.5, 0.5, 0.12) {
                pm.fill_path(&c, paint, FillRule::Winding, t, None);
            }
        }
        ProviderId::OpenRouter => {
            for pts in [
                [(0.08, 0.3), (0.5, 0.3), (0.5, 0.3)],
                [(0.08, 0.7), (0.5, 0.7), (0.5, 0.7)],
            ] {
                if let Some(p) = poly(&pts, false) {
                    stroke(pm, &p, w, paint, t);
                }
            }
            if let Some(p) = poly(&[(0.55, 0.12), (0.92, 0.5), (0.55, 0.88)], false) {
                stroke(pm, &p, w, paint, t);
            }
        }
        ProviderId::Grokbot => {
            if let Some(p) = poly(
                &[(0.12, 0.25), (0.88, 0.25), (0.88, 0.9), (0.12, 0.9)],
                true,
            ) {
                stroke(pm, &p, w, paint, t);
            }
            if let Some(l) = poly(&[(0.5, 0.25), (0.5, 0.05)], false) {
                stroke(pm, &l, w, paint, t);
            }
            for cx in [0.34, 0.66] {
                if let Some(c) = PathBuilder::from_circle(cx, 0.55, 0.07) {
                    pm.fill_path(&c, paint, FillRule::Winding, t, None);
                }
            }
        }
        ProviderId::Overview => {
            fill_rrect(pm, x, y, side, side, side * 0.28, paint);
        }
    }
}
