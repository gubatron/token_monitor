//! Hover geometry and copy for the daily bar charts (`DailyBudgetHover`).
//!
//! The Slint `HoverMath` global implements the same two functions in the
//! component; tests keep both in agreement.

/// Distance between the pointer and the floating label.
pub const POINTER_GAP: f64 = 14.0;

/// Index of the bar under `x`, or `None` for out-of-range pointers.
///
/// # Examples
///
/// ```
/// use tokenmon_ui::kit::hover::column_index;
/// assert_eq!(column_index(119.9, 40.0, 7), Some(2));
/// assert_eq!(column_index(280.0, 40.0, 7), None);
/// ```
pub fn column_index(x: f64, column_width: f64, columns: usize) -> Option<usize> {
    if columns < 1 || column_width <= 0.0 || x < 0.0 {
        return None;
    }
    let idx = (x / column_width).floor() as usize;
    (idx < columns).then_some(idx)
}

/// Top-left of the floating label for a pointer at `(px, py)`.
///
/// The label sits left of the pointer when it fits, else right, else is
/// clamped into the container. `y` is centered on the pointer and may be
/// negative (the label is not clipped).
///
/// # Examples
///
/// ```
/// use tokenmon_ui::kit::hover::label_origin;
/// assert_eq!(label_origin((200.0, 50.0), (100.0, 60.0), 300.0), (86.0, 20.0));
/// assert_eq!(label_origin((40.0, 50.0), (100.0, 60.0), 300.0), (54.0, 20.0));
/// ```
pub fn label_origin(pointer: (f64, f64), label: (f64, f64), container_width: f64) -> (f64, f64) {
    let (px, py) = pointer;
    let (w, h) = label;
    let left = px - POINTER_GAP - w;
    let right = px + POINTER_GAP;
    let x = if left >= 0.0 {
        left
    } else if right + w <= container_width {
        right
    } else {
        0.0_f64.max(left.min(container_width - w))
    };
    (x, py - h / 2.0)
}

/// Inputs of the hover usage line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DayUsage {
    /// Percentage of the period pool spent that day.
    pub spent: f64,
    /// The day's allowance as a percentage of the pool.
    pub budget: f64,
    /// Day belongs to the pool before an early provider reset.
    pub is_prior_window: bool,
}

/// The hover's second line, e.g. `31.0% of weekly · 217% of daily allowance`.
///
/// `noun` is `"weekly"` or `"monthly"`.
///
/// # Examples
///
/// ```
/// use tokenmon_ui::kit::hover::{usage_line, DayUsage};
/// let day = DayUsage { spent: 31.0, budget: 100.0 / 7.0, is_prior_window: false };
/// assert_eq!(usage_line(day, "weekly"), "31.0% of weekly · 217% of daily allowance");
/// ```
pub fn usage_line(day: DayUsage, noun: &str) -> String {
    if day.is_prior_window {
        format!(
            "{:.1}% of the {noun} pool before the provider reset it early",
            day.spent
        )
    } else if day.budget <= 0.0 {
        "No budget".to_owned()
    } else if day.spent <= 0.001 {
        format!("0% of the {:.1}% daily allowance", day.budget)
    } else {
        format!(
            "{:.1}% of {noun} · {}% of daily allowance",
            day.spent,
            (day.spent / day.budget * 100.0).round() as i64
        )
    }
}

/// Like [`label_origin`], raised so the label's bottom edge does not pass
/// `max_y` when one is given (a container near the panel's bottom, where a
/// label hanging below it would be cut off). A label taller than its
/// container may rise above the container's top.
///
/// # Examples
///
/// ```
/// use tokenmon_ui::kit::hover::label_origin_within;
/// let size = (100.0, 60.0);
/// // Near the bottom edge the label ends at the edge.
/// assert_eq!(label_origin_within((200.0, 90.0), size, 300.0, Some(100.0)).1, 40.0);
/// // Without a limit it stays centered on the pointer.
/// assert_eq!(label_origin_within((200.0, 90.0), size, 300.0, None).1, 60.0);
/// ```
pub fn label_origin_within(
    pointer: (f64, f64),
    label: (f64, f64),
    container_width: f64,
    max_y: Option<f64>,
) -> (f64, f64) {
    let (x, centered) = label_origin(pointer, label, container_width);
    let y = max_y.map_or(centered, |max| centered.min(max - label.1));
    (x, y)
}

/// A row's vertical span: its top and height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowFrame {
    /// Top edge.
    pub min_y: f64,
    /// Height.
    pub height: f64,
}

impl RowFrame {
    /// Vertical distance from `y` to the row; 0 inside it.
    fn distance(self, y: f64) -> f64 {
        if y < self.min_y {
            self.min_y - y
        } else if y > self.min_y + self.height {
            y - (self.min_y + self.height)
        } else {
            0.0
        }
    }
}

/// The row whose vertical span is nearest `y` (the one containing it, when
/// any does), so the gaps between rows keep the label up; `None` without rows.
///
/// # Examples
///
/// ```
/// use tokenmon_ui::kit::hover::{nearest_row_index, RowFrame};
/// let rows = [
///     RowFrame { min_y: 20.0, height: 30.0 },
///     RowFrame { min_y: 60.0, height: 30.0 },
/// ];
/// assert_eq!(nearest_row_index(52.0, &rows), Some(0));
/// assert_eq!(nearest_row_index(58.0, &rows), Some(1));
/// assert_eq!(nearest_row_index(10.0, &[]), None);
/// ```
pub fn nearest_row_index(y: f64, rows: &[RowFrame]) -> Option<usize> {
    rows.iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.distance(y).total_cmp(&b.distance(y)))
        .map(|(i, _)| i)
}
