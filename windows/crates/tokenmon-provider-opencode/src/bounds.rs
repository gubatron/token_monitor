//! Week and subscription-month windows (spec 03 section 3.6), UTC Gregorian.

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc};

/// A half-open `[start, end)` window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// Inclusive start.
    pub start: DateTime<Utc>,
    /// Exclusive end.
    pub end: DateTime<Utc>,
}

impl Bounds {
    /// Whether `t` lies in `[start, end)`.
    pub fn contains(&self, t: DateTime<Utc>) -> bool {
        t >= self.start && t < self.end
    }
}

/// UTC Monday 00:00 through the following Monday. Sunday belongs to the week
/// that began the previous Monday.
pub fn weekly_bounds(now: DateTime<Utc>) -> Bounds {
    let days_from_monday = i64::from(now.weekday().num_days_from_monday());
    let midnight = now.date_naive().and_hms_opt(0, 0, 0).expect("midnight");
    let start = Utc.from_utc_datetime(&midnight) - Duration::days(days_from_monday);
    Bounds {
        start,
        end: start + Duration::days(7),
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|d| d.pred_opt())
        .map_or(28, |d| d.day())
}

/// The subscription anchor's time of day placed on its day-of-month, clamped
/// to the month's length.
fn anchor(subscribed_at: DateTime<Utc>, year: i32, month: u32) -> Option<DateTime<Utc>> {
    let day = subscribed_at.day().min(days_in_month(year, month));
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let time = subscribed_at.time();
    let naive =
        date.and_hms_nano_opt(time.hour(), time.minute(), time.second(), time.nanosecond())?;
    Some(Utc.from_utc_datetime(&naive))
}

fn shift_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let idx = year * 12 + month.cast_signed() - 1 + delta;
    (idx.div_euclid(12), (idx.rem_euclid(12) + 1) as u32)
}

/// The subscription month containing `now`, anchored on `subscribed_at`'s
/// day and time (clamped to short months).
///
/// Falls back to a 30-day window starting at `now` if the arithmetic leaves
/// chrono's range, so callers never see a panic.
pub fn monthly_bounds(now: DateTime<Utc>, subscribed_at: DateTime<Utc>) -> Bounds {
    let compute = || {
        let (mut y, mut m) = (now.year(), now.month());
        let mut start = anchor(subscribed_at, y, m)?;
        if start > now {
            (y, m) = shift_month(y, m, -1);
            start = anchor(subscribed_at, y, m)?;
        }
        let (ny, nm) = shift_month(y, m, 1);
        Some(Bounds {
            start,
            end: anchor(subscribed_at, ny, nm)?,
        })
    };
    compute().unwrap_or(Bounds {
        start: now,
        end: now + Duration::days(30),
    })
}
