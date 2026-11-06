#![no_main]
//! Model-name normaliser: arbitrary Unicode never panics, variants terminate
//! and shrink, the palette index is in range.

use libfuzzer_sys::fuzz_target;
use tokenmon_provider_opencode::model_name::{display_name, model_display_name, palette_index};
use tokenmon_provider_opencode::pricing::{estimate, model_id_variants, rates};

fuzz_target!(|data: &[u8]| {
    let s = String::from_utf8_lossy(data);
    let _ = display_name(&s);
    let _ = model_display_name("opencode", &s);
    assert!(palette_index(&s) < 8);
    let variants = model_id_variants(&s);
    assert!(!variants.is_empty());
    assert!(variants.windows(2).all(|w| w[1].len() < w[0].len()));
    let _ = rates("opencode-go", &s);
    let _ = estimate("opencode", &s, Default::default());
});
