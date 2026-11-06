//! `/activity` rows grouped into per-model spend (spec 03 section 4.4).

use std::collections::HashMap;

use crate::models::ActivityRow;
use crate::snapshot::finite;

/// Prices a model's tokens when OpenRouter reports zero cost (free or stealth
/// models). The app wires the OpenCode Zen catalogue in; the default
/// [`NoCostEstimator`] never estimates.
pub trait CostEstimator: Send + Sync {
    /// Estimated USD for `prompt` and `completion` tokens of the bare model
    /// id (last `/` segment of the canonical slug), or `None` when unpriced.
    fn estimate(&self, bare_model_id: &str, prompt: i64, completion: i64) -> Option<f64>;
}

/// Estimator that prices nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoCostEstimator;

impl CostEstimator for NoCostEstimator {
    fn estimate(&self, _: &str, _: i64, _: i64) -> Option<f64> {
        None
    }
}

/// Stealth aliases later revealed as public models (lowercase key).
const REVEALED: [(&str, &str); 1] = [("stealth/ox-alpha", "z-ai/glm-5.3-flash")];

/// Maps a revealed stealth slug to its public slug; others unchanged.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_openrouter::canonical_slug;
/// assert_eq!(canonical_slug("Stealth/OX-Alpha"), "z-ai/glm-5.3-flash");
/// assert_eq!(canonical_slug("Foo/Bar"), "Foo/Bar");
/// ```
pub fn canonical_slug(slug: &str) -> String {
    let lower = slug.to_lowercase();
    REVEALED
        .iter()
        .find(|(k, _)| *k == lower)
        .map_or_else(|| slug.to_owned(), |(_, v)| (*v).to_owned())
}

/// Spend of one model over the activity window.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelUsage {
    /// Canonical slug shown to the user.
    pub model_id: String,
    /// Slug as OpenRouter reports it (first row's).
    pub activity_slug: String,
    /// Requests (negative values counted as 0).
    pub requests: i64,
    /// Prompt tokens.
    pub prompt_tokens: i64,
    /// Completion tokens.
    pub completion_tokens: i64,
    /// Cost in USD (reported, or estimated when reported is zero).
    pub cost_usd: f64,
    /// The cost came from the estimator.
    pub is_cost_estimated: bool,
    /// Share of the summed cost, `0..=100`.
    pub percent_of_window: f64,
}

impl ModelUsage {
    /// Prompt plus completion tokens.
    pub fn total_tokens(&self) -> i64 {
        self.prompt_tokens.saturating_add(self.completion_tokens)
    }

    /// The model was a stealth alias that has since been revealed.
    pub fn is_revealed(&self) -> bool {
        self.model_id != self.activity_slug
    }
}

/// Groups rows by canonical slug, estimates zero costs, sorts by cost then
/// tokens (descending) and fills the percentage shares.
pub fn models_from_rows(rows: &[ActivityRow], estimator: &dyn CostEstimator) -> Vec<ModelUsage> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut groups: Vec<ModelUsage> = Vec::new();
    for row in rows {
        let canonical = canonical_slug(&row.model);
        let i = *index.entry(canonical.clone()).or_insert_with(|| {
            groups.push(ModelUsage {
                model_id: canonical.clone(),
                activity_slug: row.model.clone(),
                requests: 0,
                prompt_tokens: 0,
                completion_tokens: 0,
                cost_usd: 0.0,
                is_cost_estimated: false,
                percent_of_window: 0.0,
            });
            groups.len() - 1
        });
        let g = &mut groups[i];
        g.requests = g.requests.saturating_add(row.requests.max(0));
        g.prompt_tokens = g.prompt_tokens.saturating_add(row.prompt_tokens.max(0));
        g.completion_tokens = g
            .completion_tokens
            .saturating_add(row.completion_tokens.max(0));
        g.cost_usd = finite(g.cost_usd + row.usage.max(0.0));
    }
    for g in &mut groups {
        apply_estimate(g, estimator);
    }
    groups.sort_by(|a, b| {
        b.cost_usd
            .total_cmp(&a.cost_usd)
            .then(b.total_tokens().cmp(&a.total_tokens()))
    });
    let total = finite(groups.iter().map(|g| g.cost_usd).sum::<f64>());
    for g in &mut groups {
        g.percent_of_window = if total > 0.0 {
            (g.cost_usd / total * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
    }
    groups
}

fn apply_estimate(g: &mut ModelUsage, estimator: &dyn CostEstimator) {
    if g.cost_usd > 0.0 {
        return;
    }
    let bare = g.model_id.rsplit('/').next().unwrap_or(&g.model_id);
    let estimate = estimator
        .estimate(bare, g.prompt_tokens, g.completion_tokens)
        .filter(|v| v.is_finite() && *v > 0.0);
    if let Some(v) = estimate {
        g.cost_usd = v;
        g.is_cost_estimated = true;
    }
}
