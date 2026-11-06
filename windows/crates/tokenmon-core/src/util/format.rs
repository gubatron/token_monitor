//! Shared value formatters (tokens, USD, hour labels, dates).

use chrono::{DateTime, Datelike, FixedOffset, Timelike, Utc};

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// Namespace for the shared formatters.
#[derive(Debug, Clone, Copy)]
pub struct Format;

fn round_half_away(value: f64, decimals: u32) -> f64 {
    let scale = if decimals == 0 { 1.0 } else { 10.0 };
    (value * scale).round() / scale
}

fn compact(value: f64, decimals: usize, suffix: &str) -> String {
    format!("{value:.decimals$}{suffix}")
}

impl Format {
    /// Compact token count (K / M / B). Each unit rounds half away from zero and
    /// a value that rounds up to 1000 of one unit is shown in the next
    /// (999,500 gives "1.0M").
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::util::Format;
    /// assert_eq!(Format::tokens(999), "999");
    /// assert_eq!(Format::tokens(1_500), "2K");
    /// assert_eq!(Format::tokens(999_500), "1.0M");
    /// assert_eq!(Format::tokens(2_500_000_000), "2.5B");
    /// ```
    #[must_use]
    pub fn tokens(count: i64) -> String {
        let value = count as f64;
        if value < 1_000.0 {
            return count.to_string();
        }
        let thousands = round_half_away(value / 1_000.0, 0);
        if thousands < 1_000.0 {
            return compact(thousands, 0, "K");
        }
        let millions = value / 1_000_000.0;
        let decimals = u32::from(round_half_away(millions, 1) < 10.0);
        let shown = round_half_away(millions, decimals);
        if shown < 1_000.0 {
            return compact(shown, decimals as usize, "M");
        }
        compact(round_half_away(value / 1_000_000_000.0, 1), 1, "B")
    }

    /// en_US currency with two decimals and a `$` symbol: 4.1 gives "$4.10",
    /// 1234.5 gives "$1,234.50", -2 gives "-$2.00". Non-finite input gives "$0".
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::util::Format;
    /// assert_eq!(Format::usd(4.1), "$4.10");
    /// assert_eq!(Format::usd(0.0), "$0.00");
    /// ```
    #[must_use]
    pub fn usd(amount: f64) -> String {
        if !amount.is_finite() {
            return "$0".to_owned();
        }
        let fixed = format!("{:.2}", amount.abs());
        let (int_part, frac) = fixed.split_once('.').unwrap_or((&fixed, "00"));
        let mut grouped = String::new();
        for (i, ch) in int_part.chars().enumerate() {
            if i > 0 && (int_part.len() - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(ch);
        }
        let negative = amount < 0.0 && (grouped != "0" || frac != "00");
        format!("{}${grouped}.{frac}", if negative { "-" } else { "" })
    }

    /// Twelve-hour clock label for a 0-23 hour: 0 gives "12a", 15 gives "3p".
    #[must_use]
    pub fn hour_label(hour: i32) -> String {
        match hour {
            0 => "12a".to_owned(),
            12 => "12p".to_owned(),
            1..=11 => format!("{hour}a"),
            _ => format!("{}p", hour - 12),
        }
    }

    /// Formats `date` in the caller-supplied `offset` using a Unicode-LDML-style
    /// `pattern` (POSIX/English names). Supported letters: `y M d E h H m s a`
    /// with their usual repeat counts, plus `'quoted'` text. Unsupported letters
    /// are copied through. Pass `clock.local_offset_at(date)` for local time.
    ///
    /// # Examples
    ///
    /// ```
    /// use chrono::{FixedOffset, TimeZone, Utc};
    /// use tokenmon_core::util::Format;
    /// let t = Utc.with_ymd_and_hms(2026, 7, 16, 18, 57, 0).unwrap();
    /// let utc = FixedOffset::east_opt(0).unwrap();
    /// assert_eq!(Format::date(t, "EEE dd MMMM h:mma", utc), "Thu 16 July 6:57PM");
    /// ```
    #[must_use]
    pub fn date(date: DateTime<Utc>, pattern: &str, offset: FixedOffset) -> String {
        render_ldml(date.with_timezone(&offset), pattern)
    }

    /// Like [`Format::date`] with `AM`/`PM` lowercased.
    #[must_use]
    pub fn reset_date(date: DateTime<Utc>, pattern: &str, offset: FixedOffset) -> String {
        Self::date(date, pattern, offset)
            .replace("AM", "am")
            .replace("PM", "pm")
    }

    /// `"Resets " + reset_date`, e.g. "Resets Thu 16 July 6:57pm" for the default pattern
    /// [`Format::DEFAULT_RESET_PATTERN`].
    #[must_use]
    pub fn reset_caption(date: DateTime<Utc>, pattern: &str, offset: FixedOffset) -> String {
        format!("Resets {}", Self::reset_date(date, pattern, offset))
    }

    /// Default pattern for [`Format::reset_caption`].
    pub const DEFAULT_RESET_PATTERN: &'static str = "EEE dd MMMM h:mma";
}

fn pad(n: i64, width: usize) -> String {
    format!("{n:0width$}")
}

fn render_run(dt: &DateTime<FixedOffset>, letter: char, n: usize) -> String {
    let hour12 = match dt.hour() % 12 {
        0 => 12,
        h => h,
    };
    let month = MONTHS[dt.month0() as usize];
    let weekday = WEEKDAYS[dt.weekday().num_days_from_monday() as usize];
    match letter {
        'y' if n == 2 => pad(i64::from(dt.year()).rem_euclid(100), 2),
        'y' => pad(i64::from(dt.year()), n),
        'M' => match n {
            1 | 2 => pad(i64::from(dt.month()), n),
            3 => month[..3].to_owned(),
            4 => month.to_owned(),
            _ => month[..1].to_owned(),
        },
        'd' => pad(i64::from(dt.day()), n.min(2)),
        'E' => match n {
            1..=3 => weekday[..3].to_owned(),
            4 => weekday.to_owned(),
            _ => weekday[..1].to_owned(),
        },
        'h' => pad(i64::from(hour12), n.min(2)),
        'H' => pad(i64::from(dt.hour()), n.min(2)),
        'm' => pad(i64::from(dt.minute()), n.min(2)),
        's' => pad(i64::from(dt.second()), n.min(2)),
        'a' => (if dt.hour() < 12 { "AM" } else { "PM" }).to_owned(),
        other => other.to_string().repeat(n),
    }
}

fn render_ldml(dt: DateTime<FixedOffset>, pattern: &str) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                out.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                out.push(chars[i]);
                i += 1;
            }
            i += 1;
        } else if c.is_ascii_alphabetic() {
            let run = chars[i..].iter().take_while(|x| **x == c).count();
            out.push_str(&render_run(&dt, c, run));
            i += run;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}
