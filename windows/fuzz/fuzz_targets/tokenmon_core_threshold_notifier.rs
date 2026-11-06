//! Drives `ThresholdNotifier` with a fuzzed record store and an operation
//! sequence; it must never panic and never alert below the threshold.
#![no_main]

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use libfuzzer_sys::fuzz_target;
use tokenmon_core::ProviderId;
use tokenmon_core::alerts::{RecordingNotifier, ThresholdNotifier};
use tokenmon_core::settings::{AppSettings, SettingsFile};

fuzz_target!(|data: &[u8]| {
    let split = data.iter().position(|b| *b == 0).unwrap_or(data.len());
    let (seed, ops) = data.split_at(split);
    let file = Arc::new(SettingsFile::from_bytes(seed));
    let settings = AppSettings::new(Arc::clone(&file));
    settings.set_threshold_enabled(true);
    let snapshot = settings.snapshot();
    let sink = Arc::new(RecordingNotifier::default());
    let notifier = ThresholdNotifier::new(file, sink.clone());
    let mut evaluations = 0usize;
    for chunk in ops.chunks_exact(4) {
        let provider = ProviderId::ALL_CASES[usize::from(chunk[0]) % ProviderId::ALL_CASES.len()];
        let used = f64::from(chunk[1]) * 0.5;
        let resets = (chunk[2] % 2 == 0)
            .then(|| {
                Utc.timestamp_opt(1_800_000_000 + i64::from(chunk[3]) * 3600, 0)
                    .single()
            })
            .flatten();
        notifier.evaluate(provider, used, &snapshot, None, resets);
        evaluations += 1;
    }
    let alerts = sink.alerts();
    assert!(alerts.len() <= evaluations);
    assert!(alerts.iter().all(|a| a.used_percent >= a.threshold));
});
