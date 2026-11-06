//! Model display names, provider short names and the colour-seed hash
//! (spec 03 section 3.9).

use tokenmon_core::Srgb;

fn brand(lower: &str) -> Option<&'static str> {
    Some(match lower {
        "gpt" => "GPT",
        "glm" => "GLM",
        "minimax" => "MiniMax",
        "deepseek" => "DeepSeek",
        "mimo" => "MiMo",
        "hy3" => "HY3",
        _ => return None,
    })
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_uppercase().chain(chars).collect::<String>()
    })
}

/// Human-readable model name, e.g. `gpt-5.4-mini` becomes `GPT-5.4 Mini`.
///
/// Never panics on arbitrary Unicode; an id with no words is returned as is.
pub fn display_name(id: &str) -> String {
    if id.to_lowercase().contains("muse-spark") {
        return "Muse Spark".to_owned();
    }
    let mut words: Vec<String> = Vec::new();
    for part in id.split('-').filter(|p| !p.is_empty()) {
        let merged = words.last().and_then(|prev| {
            if is_digits(part) && prev.chars().all(|c| c.is_ascii_digit() || c == '.') {
                Some(format!("{prev}.{part}"))
            } else if (prev == "GPT" || prev == "GLM")
                && part.chars().next().is_some_and(|c| c.is_ascii_digit())
            {
                Some(format!("{prev}-{part}"))
            } else {
                None
            }
        });
        match merged {
            Some(m) => {
                words.pop();
                words.push(m);
            }
            None => words.push(
                brand(&part.to_lowercase()).map_or_else(|| capitalize_first(part), str::to_owned),
            ),
        }
    }
    if words.is_empty() {
        id.to_owned()
    } else {
        words.join(" ")
    }
}

/// Short provider label used in model lists.
pub fn provider_short_name(provider_id: &str) -> &str {
    match provider_id {
        "opencode-go" => "Go",
        "opencode" => "Zen",
        "deepseek" => "DeepSeek",
        "xai" => "xAI",
        "openrouter" => "OpenRouter",
        "nvidia" => "NVIDIA",
        "alibaba" => "Alibaba",
        "google" => "Google",
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        other => other,
    }
}

/// `"<short> · <model>"`, with Muse Spark ids collapsed to one name.
pub fn model_display_name(provider_id: &str, model_id: &str) -> String {
    let lower = model_id.to_lowercase();
    let model = if ["muse-spark", "muse_spark", "musepark"]
        .iter()
        .any(|k| lower.contains(k))
    {
        "Muse Spark"
    } else {
        model_id
    };
    format!("{} \u{b7} {model}", provider_short_name(provider_id))
}

/// Seeded palette (spec 03 section 3.9).
pub const SEEDED_PALETTE: [(f64, f64, f64); 8] = [
    (0.55, 0.78, 1.0),
    (0.11, 0.38, 0.82),
    (0.22, 0.32, 0.48),
    (0.40, 0.55, 0.82),
    (0.75, 0.90, 1.0),
    (0.90, 0.45, 0.20),
    (0.20, 0.60, 0.40),
    (0.60, 0.30, 0.70),
];

/// Purple used for Zen and the weekly/monthly bars.
pub const PURPLE: Srgb = Srgb::rgb(0.58, 0.44, 0.86);

/// Palette index (0..8) for a seed: djb2-style wrapping hash over UTF-8 bytes.
///
/// Uses `unsigned_abs` so `i64::MIN` cannot overflow.
pub fn palette_index(seed: &str) -> usize {
    let h = seed
        .bytes()
        .fold(5381_i64, |h, b| h.wrapping_mul(33) ^ i64::from(b));
    (h.unsigned_abs() % 8) as usize
}

/// Colour for a model row: Go is the OpenCode orange, Zen purple, others seeded.
pub fn color_for_provider(provider_id: &str, seed: &str, opencode_orange: Srgb) -> Srgb {
    match provider_id.to_lowercase().as_str() {
        "opencode-go" => opencode_orange,
        "opencode" => PURPLE,
        _ => {
            let (r, g, b) = SEEDED_PALETTE[palette_index(seed)];
            Srgb::rgb(r, g, b)
        }
    }
}
