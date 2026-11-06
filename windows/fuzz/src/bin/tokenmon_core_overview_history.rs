#![no_main]
//! Fuzzes the overview history decoder and everything downstream of it.
//!
//! Invariants: decode never panics; encode/decode is a fixed point; recording
//! a saved day back into the history is idempotent; the chart built from any
//! decoded day stays inside the track.

use libfuzzer_sys::fuzz_target;
use tokenmon_core::overview::{ChartModel, OverviewHistory, ProviderDayHourlyUsage};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let history = OverviewHistory::decode(text);
    let reencoded = history.encode();
    assert_eq!(OverviewHistory::decode(&reencoded), history, "codec fixed point");
    assert_eq!(OverviewHistory::decode(&reencoded).encode(), reencoded);

    let Some(earliest) = history.earliest_day() else {
        return;
    };
    let Some(series) = history.series(earliest).cloned() else {
        return;
    };
    let mut again = history.clone();
    again.record(&series, earliest);
    assert!(!again.record(&series, earliest), "record is idempotent");

    if let Some(usage) = ProviderDayHourlyUsage::build(earliest, &series) {
        let chart = ChartModel::build(&usage);
        assert!(chart.bars.iter().all(|b| b.height <= 68.0 + 1e-9));
        assert!(
            chart
                .bars
                .iter()
                .all(|b| b.slices.iter().all(|s| s.height >= -1e-9) && b.slices.len() <= 6)
        );
    }
});
