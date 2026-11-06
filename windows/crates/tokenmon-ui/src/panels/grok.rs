//! Grok panel (`GrokPanelView`, spec 02 section 1.7).

use chrono::{DateTime, Utc};
use tokenmon_core::{ProviderId, Srgb};

use super::common::{
    DailyBudgetInput, DailyBudgetVm, FooterVm, HeaderVm, PanelContext, PanelKind, PanelVm,
    ProviderStatus, SignInAction, build_daily_budget, geometry, waiting_message,
};
use super::format;

/// A Grok product percentage (already filtered by the visibility settings).
#[derive(Debug, Clone, PartialEq)]
pub struct ProductInput {
    /// Short name (`Chat`, `Build`, ...).
    pub short_name: String,
    /// Percent of the pool.
    pub percent: f64,
    /// Product color.
    pub color: Srgb,
}

/// Grok snapshot as the panel needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct GrokSnapshotInput {
    /// Weekly used percent.
    pub used_percent: f64,
    /// Reset instant.
    pub resets_at: Option<DateTime<Utc>>,
    /// `panelProducts(...)` for the segmented bar.
    pub pool_products: Vec<ProductInput>,
    /// `categoryProducts(...)` chips (Chat, Build, Imagine, Voice order).
    pub category_products: Vec<ProductInput>,
    /// Extra credits balance in USD.
    pub extra_credits_balance: f64,
    /// Daily chart for the selected week, if any.
    pub week: Option<DailyBudgetInput>,
}

/// Grok panel input.
#[derive(Debug, Clone, PartialEq)]
pub struct GrokInput {
    /// Auth and poller state.
    pub status: ProviderStatus,
    /// `auth.isSignedIn`.
    pub is_signed_in: bool,
    /// Latest snapshot.
    pub snapshot: Option<GrokSnapshotInput>,
}

/// One laid-out segment of the pool bar.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentVm {
    /// Left edge in px.
    pub x: f64,
    /// Width in px.
    pub width: f64,
    /// Color; `None` is the grey remainder slot.
    pub color: Option<Srgb>,
}

/// Category chip.
#[derive(Debug, Clone, PartialEq)]
pub struct ChipVm {
    /// Short name.
    pub name: String,
    /// `N%`.
    pub percent_text: String,
    /// Product color.
    pub color: Srgb,
}

/// Grok usage body.
#[derive(Debug, Clone, PartialEq)]
pub struct GrokUsage {
    /// `Usage Pool Weekly`.
    pub pool_title: String,
    /// `N% Used`.
    pub pool_used_text: String,
    /// Segmented bar layout.
    pub segments: Vec<SegmentVm>,
    /// Reset caption.
    pub reset_caption: Option<String>,
    /// Category chips; `None` when the card is hidden.
    pub categories: Option<Vec<ChipVm>>,
    /// `Extra credits` amount.
    pub extra_credits: Option<String>,
    /// Daily chart.
    pub daily: Option<DailyBudgetVm>,
    /// Footer controls.
    pub footer: FooterVm,
}

/// `segmentWidths(percents, usable, minWidth)` from `SegmentedUsageBar`.
///
/// ```
/// use tokenmon_ui::panels::grok::segment_widths;
/// assert_eq!(segment_widths(&[50.0, 25.0, 25.0], 100.0, 3.0), vec![50.0, 25.0, 25.0]);
/// assert_eq!(segment_widths(&[50.0, 50.0], 0.0, 3.0), vec![3.0, 3.0]);
/// ```
pub fn segment_widths(percents: &[f64], usable: f64, min_width: f64) -> Vec<f64> {
    let mut widths: Vec<f64> = percents
        .iter()
        .map(|p| min_width.max(usable * p.max(0.0) / 100.0))
        .collect();
    let sum: f64 = widths.iter().sum();
    if usable > 0.0 && sum > usable {
        let k = usable / sum;
        for w in &mut widths {
            *w *= k;
        }
    }
    widths
}

/// Lays out the pool bar for `width` px (visible products > 0.05, remainder > 0.5).
pub fn pool_segments(products: &[ProductInput], width: f64) -> Vec<SegmentVm> {
    let visible: Vec<&ProductInput> = products.iter().filter(|p| p.percent > 0.05).collect();
    let used = visible
        .iter()
        .map(|p| p.percent.max(0.0))
        .sum::<f64>()
        .min(100.0);
    let remainder = (100.0 - used).max(0.0);
    let has_rest = remainder > 0.5;
    let slots = visible.len() + usize::from(has_rest);
    if slots == 0 {
        return Vec::new();
    }
    let gap = 2.0;
    let usable = width - gap * (slots as f64 - 1.0);
    let mut pcts: Vec<f64> = visible.iter().map(|p| p.percent).collect();
    if has_rest {
        pcts.push(remainder);
    }
    let widths = segment_widths(&pcts, usable, 3.0);
    let mut x = 0.0;
    widths
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let seg = SegmentVm {
                x,
                width: *w,
                color: visible.get(i).map(|p| p.color),
            };
            x += w + gap;
            seg
        })
        .collect()
}

/// Builds the Grok panel.
pub fn build(input: &GrokInput, ctx: &PanelContext) -> PanelVm<GrokUsage> {
    let header = HeaderVm::new(ProviderId::Grok, "SuperGrok");
    let s = &input.status;
    if s.needs_sign_in && s.has_auth_error {
        return PanelVm::frame(
            header,
            PanelKind::SessionExpired,
            Some(
                s.last_error
                    .clone()
                    .unwrap_or_else(|| "Session expired. Sign in again to load usage.".to_owned()),
            ),
            Some(SignInAction::again(ProviderId::Grok)),
        );
    }
    let Some(snap) = &input.snapshot else {
        if !input.is_signed_in {
            return PanelVm::frame(
                header,
                PanelKind::SignedOut,
                Some("Sign in to load your usage.".to_owned()),
                Some(SignInAction {
                    label: "Sign In…".to_owned(),
                    provider: ProviderId::Grok,
                }),
            );
        }
        return PanelVm::frame(
            header,
            PanelKind::Waiting,
            Some(waiting_message(s, "Signed in — waiting for usage data.")),
            s.needs_sign_in
                .then(|| SignInAction::again(ProviderId::Grok)),
        );
    };
    let categories = (!snap.category_products.is_empty()).then(|| {
        snap.category_products
            .iter()
            .map(|p| super::grok::ChipVm {
                name: p.short_name.clone(),
                percent_text: format!("{}%", p.percent.round() as i64),
                color: p.color,
            })
            .collect()
    });
    PanelVm::with_usage(
        header,
        GrokUsage {
            pool_title: "Usage Pool Weekly".to_owned(),
            pool_used_text: format::percent_used(snap.used_percent),
            segments: pool_segments(&snap.pool_products, geometry::CARD_INNER_WIDTH),
            reset_caption: snap.resets_at.map(|t| ctx.reset_caption(t)),
            categories,
            extra_credits: (snap.extra_credits_balance > 0.0)
                .then(|| format::usd(snap.extra_credits_balance)),
            daily: snap.week.as_ref().and_then(|w| build_daily_budget(w, ctx)),
            footer: FooterVm::standard(s, ctx, ProviderId::Grok, ProviderId::Grok),
        },
    )
}
