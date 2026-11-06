//! Anthropic API price table and model display names.
//!
//! Rates are USD per one million tokens, verified 2026-10-06
//! ([`RATES_VERIFIED_ON`]). Cache writes cost 1.25x input (5 minutes) or 2x
//! input (1 hour).

use crate::tokens::ModelTokens;

/// Date the table was last checked against Anthropic's price page.
pub const RATES_VERIFIED_ON: &str = "2026-10-06";

/// Per-million-token rates for one model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClaudeModelRates {
    /// Input $/1M.
    pub input: f64,
    /// Output $/1M.
    pub output: f64,
    /// Cache read $/1M.
    pub cache_read: f64,
}

impl ClaudeModelRates {
    /// 5-minute cache write $/1M (1.25 x input).
    pub fn cache_write_5m(&self) -> f64 {
        self.input * 1.25
    }
    /// 1-hour cache write $/1M (2 x input).
    pub fn cache_write_1h(&self) -> f64 {
        self.input * 2.0
    }
}

const fn r(input: f64, output: f64, cache_read: f64) -> ClaudeModelRates {
    ClaudeModelRates {
        input,
        output,
        cache_read,
    }
}

/// Exact-match table keyed by [`base_model_id`].
const TABLE: [(&str, ClaudeModelRates); 12] = [
    ("claude-fable-5-1", r(10.0, 50.0, 0.25)),
    ("claude-fable-5", r(10.0, 50.0, 1.0)),
    ("claude-opus-5-5", r(4.0, 20.0, 0.20)),
    ("claude-opus-5", r(5.0, 25.0, 0.50)),
    ("claude-opus-4-8", r(5.0, 25.0, 0.50)),
    ("claude-opus-4-7", r(5.0, 25.0, 0.50)),
    ("claude-opus-4-6", r(5.0, 25.0, 0.50)),
    ("claude-sonnet-5-5", r(2.0, 10.0, 0.20)),
    ("claude-sonnet-5", r(2.0, 10.0, 0.20)),
    ("claude-sonnet-4-6", r(3.0, 15.0, 0.30)),
    ("claude-haiku-5-5", r(0.10, 0.50, 0.01)),
    ("claude-haiku-4-5", r(1.0, 5.0, 0.10)),
];

/// Every model id in the price table (for tests and tooling).
pub fn priced_model_ids() -> impl Iterator<Item = &'static str> {
    TABLE.iter().map(|(k, _)| *k)
}

/// Lowercases `id` and drops a trailing `-YYYYMMDD` snapshot suffix.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_claude::base_model_id;
/// assert_eq!(base_model_id("claude-haiku-4-5-20251001"), "claude-haiku-4-5");
/// assert_eq!(base_model_id("Claude-Opus-5-5"), "claude-opus-5-5");
/// ```
pub fn base_model_id(id: &str) -> String {
    let lower = id.to_lowercase();
    if let Some((head, tail)) = lower.rsplit_once('-')
        && tail.len() == 8
        && tail.bytes().all(|b| b.is_ascii_digit())
    {
        return head.to_owned();
    }
    lower
}

/// Rates for `model_id`, or `None` for an unknown model.
pub fn rates(model_id: &str) -> Option<ClaudeModelRates> {
    let base = base_model_id(model_id);
    TABLE.iter().find(|(k, _)| *k == base).map(|(_, v)| *v)
}

/// API-equivalent USD cost of `t` on `model_id`, `None` when unpriced.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_claude::{cost, ModelTokens};
/// let m = 1_000_000;
/// let t = ModelTokens { input_tokens: m, output_tokens: m, cache_read_tokens: m,
///     cache_write_5m_tokens: m, cache_write_1h_tokens: m, response_count: 0 };
/// assert!((cost(&t, "claude-opus-5-5").unwrap() - 37.2).abs() < 1e-9);
/// assert!(cost(&t, "claude-unknown-9").is_none());
/// ```
pub fn cost(t: &ModelTokens, model_id: &str) -> Option<f64> {
    let p = rates(model_id)?;
    let total = t.input_tokens as f64 * p.input
        + t.output_tokens as f64 * p.output
        + t.cache_read_tokens as f64 * p.cache_read
        + t.cache_write_5m_tokens as f64 * p.cache_write_5m()
        + t.cache_write_1h_tokens as f64 * p.cache_write_1h();
    Some(total / 1_000_000.0)
}

/// Display name of a Claude model id (`claude-opus-5-5` -> `Opus 5.5`).
///
/// Uses the base id, strips a leading `claude-`, then applies the OpenCode
/// model-naming rules (spec 03 section 3.9).
///
/// # Examples
///
/// ```
/// use tokenmon_provider_claude::display_name;
/// assert_eq!(display_name("claude-fable-5-1"), "Fable 5.1");
/// assert_eq!(display_name("claude-haiku-4-5-20251001"), "Haiku 4.5");
/// ```
pub fn display_name(model_id: &str) -> String {
    let base = base_model_id(model_id);
    let stripped = base.strip_prefix("claude-").unwrap_or(&base);
    opencode_display(stripped)
}

fn brand(lower: &str) -> Option<&'static str> {
    match lower {
        "gpt" => Some("GPT"),
        "glm" => Some("GLM"),
        "minimax" => Some("MiniMax"),
        "deepseek" => Some("DeepSeek"),
        "mimo" => Some("MiMo"),
        "hy3" => Some("HY3"),
        _ => None,
    }
}

fn is_numeric_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '.')
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_uppercase().collect::<String>() + chars.as_str()
    })
}

/// `OpenCodeModelName.display` (spec 03 section 3.9). Private copy so this
/// crate does not depend on the OpenCode crate.
fn opencode_display(id: &str) -> String {
    if id.to_lowercase().contains("muse-spark") {
        return "Muse Spark".to_owned();
    }
    let words =
        id.split('-')
            .filter(|p| !p.is_empty())
            .fold(Vec::<String>::new(), |mut words, part| {
                let digits = part.bytes().all(|b| b.is_ascii_digit());
                match words.last_mut() {
                    Some(prev) if digits && is_numeric_word(prev) => {
                        prev.push('.');
                        prev.push_str(part);
                    }
                    Some(prev)
                        if (prev == "GPT" || prev == "GLM")
                            && part.chars().next().is_some_and(|c| c.is_ascii_digit()) =>
                    {
                        prev.push('-');
                        prev.push_str(part);
                    }
                    _ => words.push(
                        brand(&part.to_lowercase())
                            .map_or_else(|| capitalize_first(part), str::to_owned),
                    ),
                }
                words
            });
    if words.is_empty() {
        id.to_owned()
    } else {
        words.join(" ")
    }
}
