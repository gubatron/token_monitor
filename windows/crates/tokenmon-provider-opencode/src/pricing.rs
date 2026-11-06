//! Price tables and billable-cost logic (spec 03 section 3.5).

use crate::tables::{FALLBACK_RATES, GO_RATES, ZEN_RATES};

/// Date the rate tables were last checked against the published pricing.
pub const RATES_VERIFIED_ON: &str = "2026-10-10";

/// Provider id of the Go subscription in `opencode.db`.
pub const GO_PROVIDER: &str = "opencode-go";
/// Provider id of Zen pay-as-you-go in `opencode.db`.
pub const ZEN_PROVIDER: &str = "opencode";

/// USD price per one million tokens for each token class.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    /// Input tokens.
    pub input: f64,
    /// Output tokens.
    pub output: f64,
    /// Cache-read tokens.
    pub cache_read: f64,
    /// Cache-write tokens.
    pub cache_write: f64,
}

impl Rates {
    /// Builds a rate row.
    pub const fn new(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Self {
        Self {
            input,
            output,
            cache_read,
            cache_write,
        }
    }

    /// Dollar value of the given token counts at these rates.
    pub fn value(&self, tokens: Tokens) -> f64 {
        (self.input * tokens.input as f64
            + self.output * tokens.output as f64
            + self.cache_read * tokens.cache_read as f64
            + self.cache_write * tokens.cache_write as f64)
            / 1_000_000.0
    }
}

/// Token counts of one assistant message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tokens {
    /// Input tokens.
    pub input: i64,
    /// Output tokens.
    pub output: i64,
    /// Cache-read tokens.
    pub cache_read: i64,
    /// Cache-write tokens.
    pub cache_write: i64,
}

impl Tokens {
    /// Sum of all four classes (saturating, rows come from untrusted data).
    pub fn total(self) -> i64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
}

/// Result of [`billable_cost_usd`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Billable {
    /// Dollars to charge against the plan.
    pub cost: f64,
    /// The cost came from the price table, not from the recorded value.
    pub is_estimated: bool,
    /// A plan model with tokens but no known price.
    pub is_unpriced: bool,
}

/// Lowercased id followed by progressively stripped `-contributor` / `-free`
/// variants, e.g. `muse-spark-1.2-contributor-free` gives
/// `...-contributor-free`, `...-contributor`, `muse-spark-1.2`.
///
/// Terminates because every step shortens the string.
pub fn model_id_variants(id: &str) -> Vec<String> {
    let mut current = id.to_lowercase();
    let mut out = vec![current.clone()];
    loop {
        let stripped = current
            .strip_suffix("-contributor")
            .or_else(|| current.strip_suffix("-free"));
        match stripped {
            Some(s) => {
                current = s.to_owned();
                out.push(current.clone());
            }
            None => return out,
        }
    }
}

fn lookup(table: &[(&str, Rates)], id: &str) -> Option<Rates> {
    table.iter().find(|(k, _)| *k == id).map(|(_, r)| *r)
}

/// Rates for a (provider, model) pair: provider table first, then the shared
/// fallback, for each id variant in order.
pub fn rates(provider_id: &str, model_id: &str) -> Option<Rates> {
    let provider = provider_id.to_lowercase();
    let table: Option<&[(&str, Rates)]> = match provider.as_str() {
        GO_PROVIDER => Some(GO_RATES),
        ZEN_PROVIDER => Some(ZEN_RATES),
        _ => None,
    };
    model_id_variants(model_id).iter().find_map(|v| {
        table
            .and_then(|t| lookup(t, v))
            .or_else(|| lookup(FALLBACK_RATES, v))
    })
}

/// Rates from the shared catalogue only (used by OpenRouter).
pub fn catalog_rates(model_id: &str) -> Option<Rates> {
    model_id_variants(model_id)
        .iter()
        .find_map(|v| lookup(FALLBACK_RATES, v))
}

/// Whether the provider counts against the OpenCode plans (Go or Zen).
pub fn is_plan_provider(provider_id: &str) -> bool {
    let p = provider_id.to_lowercase();
    p == GO_PROVIDER || p == ZEN_PROVIDER
}

/// Whether the provider is the Go subscription.
pub fn is_go_provider(provider_id: &str) -> bool {
    provider_id.to_lowercase() == GO_PROVIDER
}

/// Cost to charge for a message: the recorded cost when positive, else a
/// price-table estimate for plan providers.
pub fn billable_cost_usd(
    provider_id: &str,
    model_id: &str,
    recorded: f64,
    tokens: Tokens,
) -> Billable {
    let none = |is_unpriced| Billable {
        cost: 0.0,
        is_estimated: false,
        is_unpriced,
    };
    if recorded > 0.0 {
        return Billable {
            cost: recorded,
            is_estimated: false,
            is_unpriced: false,
        };
    }
    if !is_plan_provider(provider_id) || tokens.total() <= 0 {
        return none(false);
    }
    match rates(provider_id, model_id) {
        None => none(true),
        Some(r) => {
            let v = r.value(tokens);
            Billable {
                cost: v,
                is_estimated: v > 0.0,
                is_unpriced: false,
            }
        }
    }
}

/// Estimated value of the tokens for a (provider, model), 0 when unpriced.
pub fn estimate(provider_id: &str, model_id: &str, tokens: Tokens) -> f64 {
    rates(provider_id, model_id).map_or(0.0, |r| r.value(tokens))
}

/// Estimated value using the catalogue only, 0 when unpriced.
pub fn estimate_catalog(model_id: &str, tokens: Tokens) -> f64 {
    catalog_rates(model_id).map_or(0.0, |r| r.value(tokens))
}
