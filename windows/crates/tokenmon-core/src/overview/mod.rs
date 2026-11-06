//! Overview series and the 30-day hourly history (spec 01 section 4).
//!
//! Owner: ws-overview. Everything here is pure and UI-free: the Slint panel
//! renders the plain data these modules produce.
//!
//! * [`series`]: [`OverviewSeries`], [`OverviewDaySeries`] and building today's
//!   series from the providers' hourly data.
//! * [`history`]: [`OverviewHistory`], the 30-day max-merge store with its
//!   sorted-key JSON codec.
//! * [`usage`]: [`ProviderDayHourlyUsage`], the per-hour activity model.
//! * [`chart`]: bar and slice geometry, axis labels, legend, tooltips, titles
//!   and day navigation.
//! * [`format`]: the token and hour label formatters the chart needs.

pub mod chart;
pub mod format;
pub mod history;
pub mod series;
pub mod usage;

pub use chart::{
    AXIS_HEIGHT, AxisLabel, BarModel, CORNER_RADIUS, ChartModel, EMPTY_CAPSULE_HEIGHT, LegendEntry,
    STEM_WIDTH, SliceModel, TRACK_HEIGHT, axis_hours, axis_label_text, bar_height, can_go_back,
    day_for_offset, day_title, empty_message, midnight_schedule, next_offset, previous_offset,
    segment_heights, series_color, series_legend_title, series_title, show_navigation,
    tooltip_text, visible_hours,
};
pub use history::{OverviewHistory, RETENTION_DAYS};
pub use series::{
    CursorDay, DayHourly, HOURS, OpenCodeDay, OverviewDaySeries, OverviewSeries,
    ParseOverviewSeriesError, ProviderGate, TodayInputs, build_today_series,
};
pub use usage::{HourUsage, ProviderDayHourlyUsage};
