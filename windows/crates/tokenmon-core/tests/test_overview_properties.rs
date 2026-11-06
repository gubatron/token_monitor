//! Property tests for the overview codec, merge and chart maths.

use chrono::NaiveDate;
use proptest::prelude::*;
use tokenmon_core::overview::{
    ChartModel, OverviewDaySeries, OverviewHistory, OverviewSeries, ProviderDayHourlyUsage,
    segment_heights,
};

fn series_strategy() -> impl Strategy<Value = OverviewDaySeries> {
    let w = proptest::collection::vec(0.0f64..100.0, 0..30);
    let t = proptest::collection::vec(0i64..1_000_000, 0..30);
    (
        proptest::collection::vec((0usize..6, w), 0..6),
        proptest::collection::vec((0usize..6, t), 0..6),
    )
        .prop_map(|(ws, ts)| {
            let mut s = OverviewDaySeries::default();
            for (i, v) in ws {
                s = s.with_weights(OverviewSeries::STACK_ORDER[i], v);
            }
            for (i, v) in ts {
                s = s.with_tokens(OverviewSeries::STACK_ORDER[i], v);
            }
            s
        })
}

proptest! {
    /// Decoding arbitrary text never panics, and re-encoding what was decoded
    /// is stable.
    #[test]
    fn prop_decode_is_total_and_idempotent(text in ".{0,200}") {
        let h = OverviewHistory::decode(&text);
        prop_assert_eq!(OverviewHistory::decode(&h.encode()), h);
    }

    /// Merging is commutative, idempotent and monotone.
    #[test]
    fn prop_merging_laws(a in series_strategy(), b in series_strategy()) {
        let ab = a.merging(&b);
        prop_assert_eq!(&ab, &b.merging(&a));
        prop_assert_eq!(&ab.merging(&a), &ab);
        prop_assert_eq!(&ab.merging(&b), &ab);
    }

    /// Recording the same series twice changes nothing the second time, and
    /// the history round-trips through its codec.
    #[test]
    fn prop_record_is_idempotent_and_roundtrips(s in series_strategy(), off in 0u64..40) {
        let day = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap() + chrono::Days::new(off);
        let mut h = OverviewHistory::default();
        h.record(&s, day);
        prop_assert!(!h.record(&s, day));
        let normalized = OverviewHistory::decode(&h.encode());
        prop_assert_eq!(OverviewHistory::decode(&normalized.encode()), normalized);
    }

    /// Slice heights are non-negative and fill the bar exactly.
    #[test]
    fn prop_segment_heights_fill_the_bar(
        acts in proptest::collection::vec(0.0f64..50.0, 1..7),
        total in 1.0f64..200.0,
    ) {
        let h = segment_heights(&acts, total, 1.0);
        prop_assert_eq!(h.len(), acts.len());
        prop_assert!(h.iter().all(|v| *v >= -1e-9));
        if acts.iter().any(|a| *a > 0.0) {
            prop_assert!((h.iter().sum::<f64>() - total).abs() < 1e-6);
        }
    }

    /// The chart for any series never exceeds the track and always has one bar
    /// per visible hour.
    #[test]
    fn prop_chart_bars_fit(s in series_strategy()) {
        let day = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
        if let Some(u) = ProviderDayHourlyUsage::build(day, &s) {
            let c = ChartModel::build(&u);
            prop_assert_eq!(c.bars.len(), usize::from(c.range.end() - c.range.start()) + 1);
            prop_assert!(c.bars.iter().all(|b| b.height <= 68.0 + 1e-9));
        }
    }
}
