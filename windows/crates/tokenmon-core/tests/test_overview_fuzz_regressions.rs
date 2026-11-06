//! Stable-toolchain replay of the `tokenmon_core_overview_history` fuzz corpus,
//! plus named regressions for every crash the fuzzer finds.

use std::path::Path;

use tokenmon_core::overview::{ChartModel, OverviewHistory, ProviderDayHourlyUsage};

/// The fuzz target's invariants, on a stable toolchain.
fn check_invariants(text: &str) {
    let history = OverviewHistory::decode(text);
    let reencoded = history.encode();
    assert_eq!(OverviewHistory::decode(&reencoded), history);
    assert_eq!(OverviewHistory::decode(&reencoded).encode(), reencoded);
    let Some(earliest) = history.earliest_day() else {
        return;
    };
    let series = history
        .series(earliest)
        .cloned()
        .expect("earliest day has a series");
    let mut again = history.clone();
    again.record(&series, earliest);
    assert!(!again.record(&series, earliest));
    if let Some(usage) = ProviderDayHourlyUsage::build(earliest, &series) {
        let chart = ChartModel::build(&usage);
        assert!(chart.bars.iter().all(|b| b.height <= 68.0 + 1e-9));
    }
}

/// Every committed corpus file satisfies the invariants.
#[test]
fn test_corpus_replays_cleanly() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/corpus/tokenmon_core_overview_history");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("corpus dir exists") {
        let bytes = std::fs::read(entry.expect("entry").path()).expect("read");
        if let Ok(text) = std::str::from_utf8(&bytes) {
            check_invariants(text);
            count += 1;
        }
    }
    assert!(count >= 4, "corpus has seed files");
}

/// Hand-written hostile shapes: deep nesting, huge numbers, wrong types.
#[test]
fn test_hostile_inputs_never_panic() {
    let deep = format!("{}{}", "[".repeat(5000), "]".repeat(5000));
    let big = format!(
        r#"{{"days":{{"2026-10-22":{{"weights":{{"grok":[{}]}}}}}}}}"#,
        "1e308,".repeat(50) + "1"
    );
    for text in [
        deep.as_str(),
        big.as_str(),
        r#"{"days":{"2026-10-22":{"weights":{"grok":[1e999]}}}}"#,
        r#"{"days":{"2026-10-22":{"tokens":{"cursor":[9223372036854775808]}}}}"#,
        r#"{"days":{"2026-10-22":{"tokens":{"cursor":[-9223372036854775808]}}}}"#,
        r#"{"days":{"+10000-01-01":{}, "0000-01-01":{}, "9999-12-31":{"weights":{"grok":[1]}}}}"#,
        "\u{0}",
    ] {
        check_invariants(text);
    }
}
