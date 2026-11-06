//! Domain model of the SuperGrok weekly pool (spec 02 section 1.3).
//!
//! The JSON shape of [`WeeklyUsageSnapshot`] matches the macOS app's
//! `grok_history.dat` rows (camelCase keys, dates as seconds since
//! 2001-01-01 UTC), so a history file written on either platform decodes on
//! the other.

use std::collections::HashSet;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Clamps a percentage into `0.0..=100.0`; NaN becomes `0.0`.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::models::clamp_percent;
/// assert_eq!(clamp_percent(140.0), 100.0);
/// assert_eq!(clamp_percent(-5.0), 0.0);
/// assert_eq!(clamp_percent(f64::NAN), 0.0);
/// ```
pub fn clamp_percent(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 100.0)
    }
}

/// Semantic color token of a product segment (`ProductColor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProductColor {
    /// Grok Build.
    Build,
    /// API.
    Api,
    /// Chat.
    Chat,
    /// Imagine.
    Imagine,
    /// Voice.
    Voice,
    /// Anything else.
    Other,
}

impl ProductColor {
    /// Color token for a product id, case-insensitive.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_provider_grok::models::ProductColor;
    /// assert_eq!(ProductColor::from_product_id("API"), ProductColor::Api);
    /// assert_eq!(ProductColor::from_product_id("grok-build"), ProductColor::Build);
    /// assert_eq!(ProductColor::from_product_id("mystery"), ProductColor::Other);
    /// ```
    pub fn from_product_id(id: &str) -> Self {
        match id.to_lowercase().as_str() {
            "build" | "grok_build" | "grok-build" | "grok build" => Self::Build,
            "api" => Self::Api,
            "chat" | "grok" | "text" => Self::Chat,
            "imagine" | "image" | "video" | "media" => Self::Imagine,
            "voice" => Self::Voice,
            _ => Self::Other,
        }
    }

    /// sRGB components `(r, g, b)` in `0.0..=1.0`.
    pub const fn srgb(self) -> (f64, f64, f64) {
        match self {
            Self::Chat => (0.11, 0.38, 0.82),
            Self::Build => (0.55, 0.78, 1.0),
            Self::Voice => (0.40, 0.55, 0.82),
            Self::Api => (0.22, 0.32, 0.48),
            Self::Imagine => (0.75, 0.90, 1.0),
            Self::Other => (0.45, 0.45, 0.45),
        }
    }
}

/// Canonical product ids, labels and display order (`ProductCatalog`).
pub struct ProductCatalog;

impl ProductCatalog {
    /// Preference toggle order; also the default visible set.
    pub const KNOWN_IDS: [&'static str; 6] = ["build", "api", "chat", "imagine", "voice", "other"];
    /// Visual priority for bars, chips and legends.
    pub const DISPLAY_ORDER: [&'static str; 6] =
        ["chat", "build", "imagine", "voice", "api", "other"];
    /// Ids of the Categories card (API and Other never appear there).
    pub const CATEGORY_IDS: [&'static str; 4] = ["chat", "build", "imagine", "voice"];

    /// Index of `id` (lowercased) in [`Self::DISPLAY_ORDER`]; `99` when unknown.
    pub fn display_index(id: &str) -> usize {
        let lower = id.to_lowercase();
        Self::DISPLAY_ORDER
            .iter()
            .position(|known| *known == lower)
            .unwrap_or(99)
    }

    /// Long display name; unknown ids are capitalized per word like
    /// Foundation's `.capitalized`.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_provider_grok::models::ProductCatalog;
    /// assert_eq!(ProductCatalog::display_name("build"), "Grok Build");
    /// assert_eq!(ProductCatalog::display_name("product-9"), "Product-9");
    /// ```
    pub fn display_name(id: &str) -> String {
        match id.to_lowercase().as_str() {
            "build" => "Grok Build".into(),
            "api" => "API".into(),
            "chat" => "Chat".into(),
            "imagine" => "Imagine".into(),
            "voice" => "Voice".into(),
            "other" => "Other".into(),
            _ => capitalized(id),
        }
    }

    /// Short display name (`Build` instead of `Grok Build`).
    pub fn short_name(id: &str) -> String {
        match id.to_lowercase().as_str() {
            "build" => "Build".into(),
            _ => Self::display_name(id),
        }
    }

    /// Stable sort into display order, ties broken by id.
    ///
    /// Swift's tie order for unknown ids is nondeterministic; the port is
    /// deterministic (spec section 4, divergence 3).
    pub fn sort_for_display(products: &mut [ProductUsage]) {
        products.sort_by(|a, b| {
            Self::display_index(&a.id)
                .cmp(&Self::display_index(&b.id))
                .then_with(|| a.id.cmp(&b.id))
        });
    }

    /// Products the user can see that contribute more than `threshold`, in
    /// display order. Ids outside [`Self::DISPLAY_ORDER`] are dropped, and
    /// later duplicates win.
    pub fn filtered(
        products: &[ProductUsage],
        visible: &HashSet<String>,
        threshold: f64,
    ) -> Vec<ProductUsage> {
        Self::DISPLAY_ORDER
            .iter()
            .filter(|id| visible.contains(**id))
            .filter_map(|id| products.iter().rev().find(|p| p.id.to_lowercase() == *id))
            .filter(|p| p.percent_of_pool > threshold)
            .cloned()
            .collect()
    }

    /// Products for the panel bar and category rows (any non-zero slice).
    pub fn panel_products(
        products: &[ProductUsage],
        visible: &HashSet<String>,
    ) -> Vec<ProductUsage> {
        Self::filtered(products, visible, 0.0)
    }

    /// One row per visible canonical category, with a 0% placeholder when the
    /// product is absent.
    pub fn category_products(
        products: &[ProductUsage],
        visible: &HashSet<String>,
    ) -> Vec<ProductUsage> {
        Self::CATEGORY_IDS
            .iter()
            .filter(|id| visible.contains(**id))
            .map(|id| {
                products
                    .iter()
                    .rev()
                    .find(|p| p.id.to_lowercase() == *id)
                    .cloned()
                    .unwrap_or_else(|| ProductUsage::new(id, Self::display_name(id), 0.0))
            })
            .collect()
    }

    /// The persisted visible-product set after normalization: lowercased and
    /// intersected with [`Self::KNOWN_IDS`]; a non-empty stored set that
    /// normalizes to nothing falls back to all known ids, an explicitly empty
    /// one stays empty.
    pub fn normalize_visible_ids(stored: &[String]) -> HashSet<String> {
        let all = || Self::KNOWN_IDS.iter().map(|s| (*s).to_owned()).collect();
        let kept: HashSet<String> = stored
            .iter()
            .map(|s| s.to_lowercase())
            .filter(|s| Self::KNOWN_IDS.contains(&s.as_str()))
            .collect();
        if kept.is_empty() && !stored.is_empty() {
            all()
        } else {
            kept
        }
    }
}

/// Foundation-style `.capitalized`: first letter of each word upper, rest lower.
fn capitalized(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start_of_word = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if start_of_word {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start_of_word = false;
        } else {
            out.push(c);
            start_of_word = true;
        }
    }
    out
}

/// A product's contribution to the weekly pool, always clamped to 0..=100.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "ProductUsageRaw")]
pub struct ProductUsage {
    /// Lowercase product id (`chat`, `build`, `product-9`).
    pub id: String,
    /// Label for the UI.
    pub display_name: String,
    /// Share of the pool, `0.0..=100.0`.
    pub percent_of_pool: f64,
    /// Color token.
    pub color_token: ProductColor,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProductUsageRaw {
    id: String,
    display_name: String,
    percent_of_pool: f64,
    color_token: ProductColor,
}

impl From<ProductUsageRaw> for ProductUsage {
    fn from(raw: ProductUsageRaw) -> Self {
        Self {
            id: raw.id,
            display_name: raw.display_name,
            percent_of_pool: clamp_percent(raw.percent_of_pool),
            color_token: raw.color_token,
        }
    }
}

impl ProductUsage {
    /// Builds a product, clamping the percentage and deriving the color from
    /// the id.
    pub fn new(id: &str, display_name: impl Into<String>, percent_of_pool: f64) -> Self {
        Self {
            id: id.to_owned(),
            display_name: display_name.into(),
            percent_of_pool: clamp_percent(percent_of_pool),
            color_token: ProductColor::from_product_id(id),
        }
    }

    /// Like [`Self::new`] with an explicit color token.
    pub fn with_color(mut self, color_token: ProductColor) -> Self {
        self.color_token = color_token;
        self
    }
}

/// A finite decimal amount (extra credits) kept as its shortest written form.
///
/// Serialized as a JSON number. Non-finite values cannot be constructed.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::models::CreditBalance;
/// assert_eq!(CreditBalance::from_f64(4.1).unwrap().to_string(), "4.1");
/// assert!(CreditBalance::from_f64(f64::NAN).is_none());
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct CreditBalance(f64);

impl CreditBalance {
    /// A balance, or `None` when `value` is NaN or infinite.
    pub fn from_f64(value: f64) -> Option<Self> {
        value.is_finite().then_some(Self(value))
    }

    /// The numeric value.
    pub fn value(&self) -> f64 {
        self.0
    }
}

impl fmt::Display for CreditBalance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for CreditBalance {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for CreditBalance {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = f64::deserialize(d)?;
        Self::from_f64(v).ok_or_else(|| serde::de::Error::custom("non-finite credit balance"))
    }
}

/// Serde adapter for dates stored as seconds since 2001-01-01T00:00:00Z
/// (Swift's default `JSONEncoder` `Date` strategy).
pub mod apple_date {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    /// Seconds between the Unix epoch and 2001-01-01T00:00:00Z.
    pub const APPLE_EPOCH_UNIX: i64 = 978_307_200;

    /// Converts an instant to Apple reference-date seconds (microsecond precision).
    pub fn to_apple(at: DateTime<Utc>) -> f64 {
        (at.timestamp_micros() as f64) / 1_000_000.0 - APPLE_EPOCH_UNIX as f64
    }

    /// Converts Apple reference-date seconds to an instant; `None` when the
    /// value is not finite or out of range.
    pub fn from_apple(secs: f64) -> Option<DateTime<Utc>> {
        if !secs.is_finite() {
            return None;
        }
        let micros = ((secs + APPLE_EPOCH_UNIX as f64) * 1_000_000.0).round();
        if micros.abs() > 9.0e18 {
            return None;
        }
        DateTime::from_timestamp_micros(micros as i64)
    }

    /// `serialize_with` for a required date.
    pub fn serialize<S: Serializer>(at: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(to_apple(*at))
    }

    /// `deserialize_with` for a required date.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Utc>, D::Error> {
        let secs = f64::deserialize(d)?;
        from_apple(secs).ok_or_else(|| serde::de::Error::custom("date out of range"))
    }

    /// Adapter for `Option<DateTime<Utc>>`.
    pub mod option {
        use chrono::{DateTime, Utc};
        use serde::{Deserialize, Deserializer, Serializer};

        /// `serialize_with` for an optional date (use with `skip_serializing_if`).
        pub fn serialize<S: Serializer>(
            at: &Option<DateTime<Utc>>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match at {
                Some(at) => s.serialize_f64(super::to_apple(*at)),
                None => s.serialize_none(),
            }
        }

        /// `deserialize_with` for an optional date.
        pub fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<DateTime<Utc>>, D::Error> {
            match Option::<f64>::deserialize(d)? {
                None => Ok(None),
                Some(secs) => super::from_apple(secs)
                    .map(Some)
                    .ok_or_else(|| serde::de::Error::custom("date out of range")),
            }
        }
    }
}

/// Snapshot of the weekly SuperGrok pool at a point in time.
///
/// # Examples
///
/// ```
/// use chrono::Utc;
/// use tokenmon_provider_grok::models::WeeklyUsageSnapshot;
/// let s = WeeklyUsageSnapshot::new(Utc::now(), 40.0);
/// assert_eq!(s.remaining_percent, 60.0);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeeklyUsageSnapshot {
    /// New for every poll.
    pub id: Uuid,
    /// When the snapshot was fetched.
    #[serde(with = "apple_date")]
    pub fetched_at: DateTime<Utc>,
    /// Used share of the pool, `0.0..=100.0`.
    pub used_percent: f64,
    /// Remaining share of the pool, `0.0..=100.0`.
    pub remaining_percent: f64,
    /// When the pool resets.
    #[serde(
        default,
        with = "apple_date::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub resets_at: Option<DateTime<Utc>>,
    /// Product breakdown.
    pub products: Vec<ProductUsage>,
    /// Extra credits balance, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_credits_balance: Option<CreditBalance>,
    /// Account email, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
}

impl WeeklyUsageSnapshot {
    /// Grok's allowance is always a weekly pool.
    pub fn usage_pool(&self) -> tokenmon_core::usage::UsagePool {
        tokenmon_core::usage::UsagePool::Weekly
    }

    /// A snapshot with `remaining = 100 - used`, no reset, products or account.
    pub fn new(fetched_at: DateTime<Utc>, used_percent: f64) -> Self {
        let used = clamp_percent(used_percent);
        Self {
            id: Uuid::new_v4(),
            fetched_at,
            used_percent: used,
            remaining_percent: clamp_percent(100.0 - used),
            resets_at: None,
            products: Vec::new(),
            extra_credits_balance: None,
            account_email: None,
        }
    }

    /// Replaces the remaining percentage (clamped).
    #[must_use]
    pub fn with_remaining(mut self, remaining: f64) -> Self {
        self.remaining_percent = clamp_percent(remaining);
        self
    }

    /// Sets the reset instant.
    #[must_use]
    pub fn with_resets_at(mut self, resets_at: Option<DateTime<Utc>>) -> Self {
        self.resets_at = resets_at;
        self
    }

    /// Sets the product breakdown.
    #[must_use]
    pub fn with_products(mut self, products: Vec<ProductUsage>) -> Self {
        self.products = products;
        self
    }

    /// Sets the account email.
    #[must_use]
    pub fn with_account(mut self, email: Option<String>) -> Self {
        self.account_email = email;
        self
    }

    /// Sets the extra credits balance.
    #[must_use]
    pub fn with_credits(mut self, credits: Option<CreditBalance>) -> Self {
        self.extra_credits_balance = credits;
        self
    }
}

/// Without a product breakdown, attributes the pool to a neutral `other` slice.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::models::synthesize_products;
/// assert_eq!(synthesize_products(64.0)[0].id, "other");
/// assert!(synthesize_products(0.01).is_empty());
/// ```
pub fn synthesize_products(used_percent: f64) -> Vec<ProductUsage> {
    if used_percent > 0.05 {
        vec![ProductUsage::new("other", "Other", used_percent).with_color(ProductColor::Other)]
    } else {
        Vec::new()
    }
}

/// One product slice of a single day's contribution to the weekly pool.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyUsageSegment {
    /// Product id.
    pub product_id: String,
    /// Label.
    pub display_name: String,
    /// Share of the weekly pool.
    pub percent_of_weekly: f64,
    /// Color token.
    pub color_token: ProductColor,
}

/// One calendar day of the daily chart.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyUsageDay {
    /// Local midnight of the day.
    pub day_start: DateTime<Utc>,
    /// First three letters of the weekday (`Thu`).
    pub weekday_symbol: String,
    /// Day of month (`16`).
    pub day_of_month: String,
    /// Product segments; empty means no bar.
    pub segments: Vec<DailyUsageSegment>,
    /// The day contains `now`.
    pub is_today: bool,
    /// A period rollover or mid-period rebase started on this day.
    pub is_after_reset: bool,
}

impl DailyUsageDay {
    /// Sum of the segments.
    pub fn total_percent(&self) -> f64 {
        self.segments.iter().map(|s| s.percent_of_weekly).sum()
    }
}

/// Legend entry for the daily chart.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyUsageLegendItem {
    /// Product id.
    pub id: String,
    /// Label.
    pub display_name: String,
    /// Color token.
    pub color_token: ProductColor,
}

/// Billing-period window of the daily chart.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyUsageWeek {
    /// First day (local midnight).
    pub week_start: DateTime<Utc>,
    /// Last day (local midnight); the 8th bar on the reset morning.
    pub week_end: DateTime<Utc>,
    /// Chronological days.
    pub days: Vec<DailyUsageDay>,
    /// Products seen in any day, in display order.
    pub legend_products: Vec<DailyUsageLegendItem>,
    /// At least one day has a real delta.
    pub has_daily_data: bool,
    /// Fewer than two in-week sample days exist.
    pub is_estimated: bool,
}

/// Equal daily share of the weekly pool.
pub const DAILY_CAP_PERCENT: f64 = 100.0 / 7.0;

/// Maps a day's weekly-pool percent into `0..=1` of the daily track.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::models::fill_fraction;
/// assert_eq!(fill_fraction(21.0), 1.0);
/// assert_eq!(fill_fraction(0.0), 0.0);
/// ```
pub fn fill_fraction(day_percent: f64) -> f64 {
    if day_percent <= 0.05 {
        0.0
    } else {
        (day_percent / DAILY_CAP_PERCENT).min(1.0)
    }
}
