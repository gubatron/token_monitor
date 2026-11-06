#![no_main]
//! Console JSON parsers: arbitrary bytes must never panic, percents stay in
//! `[0, 100]` and finite, and org ids always carry the `wrk_` prefix.

use libfuzzer_sys::fuzz_target;
use tokenmon_provider_opencode::console::{
    parse_go_meters, parse_orgs, parse_session_email, workspace_id_from_path,
};

fuzz_target!(|data: &[u8]| {
    if let Ok(meters) = parse_go_meters(data) {
        let snap = meters.into_snapshot();
        for w in &snap.windows {
            let p = w.used_percent();
            assert!(p.is_finite() && (0.0..=100.0).contains(&p));
        }
        let p = snap.monthly_used_percent();
        assert!(p.is_finite() && (0.0..=100.0).contains(&p));
    }
    if let Ok(ids) = parse_orgs(data) {
        assert!(ids.iter().all(|i| i.starts_with("wrk_")));
    }
    let _ = parse_session_email(data);
    if let Ok(s) = std::str::from_utf8(data) {
        if let Some(id) = workspace_id_from_path(s) {
            assert!(id.starts_with("wrk_"));
        }
    }
});
