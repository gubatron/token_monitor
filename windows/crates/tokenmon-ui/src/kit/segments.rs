//! Segment geometry for `SegmentedUsageBar` (spec 04 section 3.4).

use slint::Color;
use tokenmon_core::Srgb;

use crate::kit::palette::to_color;
use crate::kit::ui::BarSegment;

/// Gap between segments, in px.
pub const SEGMENT_GAP: f64 = 2.0;
/// Narrowest a visible segment may be, in px.
pub const MIN_SEGMENT_WIDTH: f64 = 3.0;
/// Products at or below this share of the pool are not drawn.
pub const MIN_VISIBLE_PERCENT: f64 = 0.05;
/// The empty remainder slot is only drawn above this percentage.
pub const MIN_REMAINDER_PERCENT: f64 = 0.5;

/// One product's share of a usage pool.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductShare {
    /// Display name (used by [`CategoryRow`](crate::kit::ui::CategoryItem) legends).
    pub name: String,
    /// Percentage of the pool, 0...100.
    pub percent_of_pool: f64,
    /// Product color.
    pub color: Srgb,
}

/// A laid-out slot of the bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentSpec {
    /// Width in px.
    pub width: f64,
    /// Fill color (ignored for the remainder slot).
    pub color: Srgb,
    /// Whether this is the trailing "remaining" slot.
    pub remainder: bool,
}

/// Width of each segment: `max(min_width, usable * p / 100)`, scaled down
/// uniformly when the total exceeds `usable` (and `usable > 0`).
///
/// # Examples
///
/// ```
/// use tokenmon_ui::kit::segments::segment_widths;
/// assert_eq!(segment_widths(&[50.0, 25.0, 25.0], 100.0, 3.0), vec![50.0, 25.0, 25.0]);
/// assert_eq!(segment_widths(&[50.0, 50.0], 0.0, 3.0), vec![3.0, 3.0]);
/// ```
pub fn segment_widths(percents: &[f64], usable: f64, min_width: f64) -> Vec<f64> {
    let floored: Vec<f64> = percents
        .iter()
        .map(|p| min_width.max(usable * p.max(0.0) / 100.0))
        .collect();
    let total: f64 = floored.iter().sum();
    if usable > 0.0 && total > usable {
        let scale = usable / total;
        floored.iter().map(|w| w * scale).collect()
    } else {
        floored
    }
}

/// Lays out the bar for a track `width` px wide: visible products, then the
/// remainder slot when more than 0.5% of the pool is unused.
pub fn layout_segments(
    products: &[ProductShare],
    width: f64,
    remainder_color: Srgb,
) -> Vec<SegmentSpec> {
    let visible: Vec<&ProductShare> = products
        .iter()
        .filter(|p| p.percent_of_pool > MIN_VISIBLE_PERCENT)
        .collect();
    let used = visible
        .iter()
        .map(|p| p.percent_of_pool.max(0.0))
        .sum::<f64>()
        .min(100.0);
    let remainder = (100.0 - used).max(0.0);
    let has_remainder = remainder > MIN_REMAINDER_PERCENT;
    let slots = visible.len() + usize::from(has_remainder);
    let usable = width - SEGMENT_GAP * slots.saturating_sub(1) as f64;
    let percents: Vec<f64> = visible
        .iter()
        .map(|p| p.percent_of_pool)
        .chain(has_remainder.then_some(remainder))
        .collect();
    segment_widths(&percents, usable, MIN_SEGMENT_WIDTH)
        .into_iter()
        .enumerate()
        .map(|(i, w)| match visible.get(i) {
            Some(p) => SegmentSpec {
                width: w,
                color: p.color,
                remainder: false,
            },
            None => SegmentSpec {
                width: w,
                color: remainder_color,
                remainder: true,
            },
        })
        .collect()
}

/// Converts laid-out segments to the Slint struct model for `SegmentedUsageBar`.
pub fn to_bar_segments(specs: &[SegmentSpec]) -> slint::ModelRc<BarSegment> {
    let items: Vec<BarSegment> = specs
        .iter()
        .map(|s| BarSegment {
            width: s.width as f32,
            color: to_color(s.color),
            remainder: s.remainder,
        })
        .collect();
    slint::ModelRc::new(slint::VecModel::from(items))
}

/// Opaque color helper for tests and callers building shares from Slint colors.
pub fn color_of(rgb: (u8, u8, u8)) -> Color {
    Color::from_rgb_u8(rgb.0, rgb.1, rgb.2)
}
