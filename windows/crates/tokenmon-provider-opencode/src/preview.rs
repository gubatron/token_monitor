//! Fixed sample data for previews and design work (`OpenCodeSnapshot.preview`).

use chrono::{DateTime, Duration, Utc};

use crate::snapshot::{ModelUsage, OpenCodeSnapshot, WindowKind, WindowUsage};

fn window(
    now: DateTime<Utc>,
    kind: WindowKind,
    used_usd: f64,
    limit_usd: f64,
    resets_in: Duration,
    session_count: u32,
) -> WindowUsage {
    WindowUsage {
        kind,
        used_usd,
        limit_usd,
        resets_at: Some(now + resets_in),
        session_count,
    }
}

fn model(
    model_id: &str,
    session_count: u32,
    tokens: (i64, i64, i64),
    cost_usd: f64,
    percent_of_window: f64,
) -> ModelUsage {
    ModelUsage {
        provider_id: "opencode-go".to_owned(),
        model_id: model_id.to_owned(),
        cost_usd,
        input_tokens: tokens.0,
        output_tokens: tokens.1,
        cache_read_tokens: tokens.2,
        cache_write_tokens: 0,
        session_count,
        percent_of_window,
        is_cost_estimated: false,
        is_cost_unpriced: false,
    }
}

impl OpenCodeSnapshot {
    /// Sample snapshot with three windows (5-hour, weekly, monthly) and two
    /// models. Reset times are relative to `now` so the sample never looks
    /// stale.
    ///
    /// # Examples
    ///
    /// ```
    /// use chrono::Utc;
    /// use tokenmon_provider_opencode::snapshot::{OpenCodeSnapshot, WindowKind};
    ///
    /// let preview = OpenCodeSnapshot::preview(Utc::now());
    /// assert_eq!(preview.windows.len(), 3);
    /// assert_eq!(preview.windows[0].kind, WindowKind::Rolling5h);
    /// ```
    pub fn preview(now: DateTime<Utc>) -> Self {
        Self {
            windows: vec![
                window(
                    now,
                    WindowKind::Rolling5h,
                    8.6,
                    12.0,
                    Duration::hours(2),
                    14,
                ),
                window(now, WindowKind::Weekly, 12.4, 30.0, Duration::days(3), 41),
                window(
                    now,
                    WindowKind::Monthly,
                    16.9,
                    60.0,
                    Duration::days(17),
                    120,
                ),
            ],
            models: vec![
                model(
                    "minimax-m3",
                    9,
                    (14_200_000, 1_300_000, 512_000_000),
                    4.1,
                    48.0,
                ),
                model("kimi-k3", 3, (8_900_000, 420_000, 301_000_000), 2.6, 30.0),
            ],
            is_estimated: false,
            monthly_tokens: 48_000_000,
            monthly_estimated_usd: 22.4,
            monthly_input_tokens: 35_000_000,
            monthly_output_tokens: 8_500_000,
        }
    }
}
