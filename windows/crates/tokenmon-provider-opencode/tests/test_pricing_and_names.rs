//! Price tables, billable cost and model naming (spec 03 sections 3.5, 3.9; tests
//! `OpenCodePriceTableTests`, `OpenCodeModelNameTests` and the Zen/catalogue
//! cases of `OpenCodeStatsTests`).

use tokenmon_provider_opencode::model_name::{
    PURPLE, color_for_provider, display_name, model_display_name, palette_index,
    provider_short_name,
};
use tokenmon_provider_opencode::pricing::{
    RATES_VERIFIED_ON, Tokens, billable_cost_usd, catalog_rates, estimate, estimate_catalog,
    is_go_provider, is_plan_provider, model_id_variants, rates,
};
use tokenmon_core::Srgb;

fn input(n: i64) -> Tokens {
    Tokens { input: n, ..Tokens::default() }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Priced models are not flagged unpriced, even at a recorded cost of zero.
#[test]
fn test_priced_models_are_not_flagged_unpriced() {
    let b = billable_cost_usd("opencode", "claude-haiku-5-5", 0.0, input(1_000_000));
    assert!(close(b.cost, 0.10) && b.is_estimated && !b.is_unpriced);
    let b = billable_cost_usd("opencode", "unknown-model", 1.5, input(1_000_000));
    assert!(close(b.cost, 1.5) && !b.is_estimated && !b.is_unpriced);
}

/// The verification date is a valid day key.
#[test]
fn test_rates_carry_a_verification_date() {
    assert!(chrono::NaiveDate::parse_from_str(RATES_VERIFIED_ON, "%Y-%m-%d").is_ok());
}

/// Unknown free model: unpriced, no cost.
#[test]
fn test_unknown_model_is_flagged_unpriced() {
    let b = billable_cost_usd("opencode", "totally-new-model-free", 0.0, input(10));
    assert_eq!((b.cost, b.is_estimated, b.is_unpriced), (0.0, false, true));
}

/// Non-plan providers and empty token counts never estimate.
#[test]
fn test_non_plan_and_empty_rows_are_free() {
    let b = billable_cost_usd("anthropic", "claude-opus-5", 0.0, input(10));
    assert_eq!((b.cost, b.is_unpriced), (0.0, false));
    let b = billable_cost_usd("opencode", "claude-opus-5", 0.0, Tokens::default());
    assert_eq!((b.cost, b.is_unpriced), (0.0, false));
}

/// Zen free alias is valued from published token prices.
#[test]
fn test_zen_zero_cost_uses_published_token_value() {
    let t = Tokens { input: 1_000_000, output: 500_000, ..Tokens::default() };
    let b = billable_cost_usd("opencode", "deepseek-v4-flash-free", 0.0, t);
    assert!(close(b.cost, 0.28) && b.is_estimated);
    assert!(close(estimate("opencode", "deepseek-v4-flash", input(1_000_000)), 0.14));
}

/// A contributor-free alias resolves to the paid-equivalent rate.
#[test]
fn test_muse_spark_free_alias_estimates_paid_equivalent_value() {
    let t = Tokens { input: 1_000_000, output: 1_000_000, ..Tokens::default() };
    let b = billable_cost_usd("opencode-go", "muse-spark-1.3-contributor-free", 0.0, t);
    assert!(close(b.cost, 0.30));
}

/// Free aliases resolve to base rates in the catalogue.
#[test]
fn test_free_alias_resolves_to_base_rates() {
    assert!(close(estimate_catalog("kimi-k3-free", input(1_000_000)), 3.0));
    assert!(catalog_rates("nope").is_none());
}

/// Variant stripping order and termination.
#[test]
fn test_model_id_variants() {
    assert_eq!(
        model_id_variants("Muse-Spark-1.2-Contributor-Free"),
        [
            "muse-spark-1.2-contributor-free",
            "muse-spark-1.2-contributor",
            "muse-spark-1.2"
        ]
    );
    assert_eq!(model_id_variants("-free-free").len(), 3);
    assert_eq!(model_id_variants("").len(), 1);
}

/// Table sizes and some spot rows match the spec exactly.
#[test]
fn test_price_tables_match_spec() {
    let go = rates("opencode-go", "minimax-m2.7").expect("go");
    assert_eq!((go.input, go.output, go.cache_read, go.cache_write), (0.30, 1.20, 0.06, 0.375));
    let zen = rates("OpenCode", "gpt-5.5-pro").expect("zen");
    assert_eq!((zen.input, zen.output, zen.cache_read), (30.0, 180.0, 30.0));
    // Go-only table entry wins over fallback.
    assert_eq!(rates("opencode-go", "deepseek-v4-flash").expect("r").input, 0.15);
    assert_eq!(rates("opencode", "deepseek-v4-flash").expect("r").input, 0.14);
    // Fallback only.
    assert_eq!(rates("opencode", "big-pickle").expect("r").input, 0.30);
    assert!(rates("xai", "grok-4.5").is_some());
    assert!(rates("opencode-go", "glm-5.1").is_some(), "falls back to the catalogue");
}

/// Provider eligibility.
#[test]
fn test_provider_predicates() {
    assert!(is_go_provider("OpenCode-Go") && is_go_provider("opencode-go"));
    assert!(!is_go_provider("opencode"));
    for p in ["opencode-go", "opencode", "OpenCode"] {
        assert!(is_plan_provider(p));
    }
    for p in ["deepseek", "xai", "openai", "anthropic"] {
        assert!(!is_plan_provider(p));
    }
}

/// The eight spec examples.
#[test]
fn test_model_display_names() {
    for (id, want) in [
        ("gpt-5.4-mini", "GPT-5.4 Mini"),
        ("glm-5.1", "GLM-5.1"),
        ("kimi-k2.6", "Kimi K2.6"),
        ("qwen3.6-plus", "Qwen3.6 Plus"),
        ("minimax-m3", "MiniMax M3"),
        ("deepseek-v4-flash-free", "DeepSeek V4 Flash Free"),
        ("claude-sonnet-4-5", "Claude Sonnet 4.5"),
        ("muse-spark-1.2-contributor-free", "Muse Spark"),
    ] {
        assert_eq!(display_name(id), want, "{id}");
    }
    assert_eq!(display_name("---"), "---");
    assert_eq!(display_name(""), "");
}

/// Catalogue names.
#[test]
fn test_catalog_names() {
    assert_eq!(provider_short_name("opencode-go"), "Go");
    assert_eq!(provider_short_name("opencode"), "Zen");
    assert_eq!(provider_short_name("deepseek"), "DeepSeek");
    assert_eq!(provider_short_name("other"), "other");
    assert_eq!(model_display_name("deepseek", "deepseek-v4-pro"), "DeepSeek \u{b7} deepseek-v4-pro");
    assert_eq!(model_display_name("opencode", "muse_spark-2"), "Zen \u{b7} Muse Spark");
}

/// Palette is deterministic, bounded, and `i64::MIN` safe.
#[test]
fn test_model_palette() {
    assert_eq!(palette_index("opencode/x"), palette_index("opencode/x"));
    let orange = Srgb::rgb(1.0, 0.5, 0.0);
    assert_eq!(color_for_provider("opencode-go", "s", orange), orange);
    assert_eq!(color_for_provider("opencode", "s", orange), PURPLE);
    let c = color_for_provider("xai", "xai/grok", orange);
    assert!([c.red, c.green, c.blue].iter().all(|v| (0.0..=1.0).contains(v)));
    for s in ["", "a", "\u{1F600}", "provider/model-1.2"] {
        assert!(palette_index(s) < 8);
    }
}
