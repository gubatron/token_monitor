//! Invariants over any event sequence: `Ready` is only produced while a wake
//! is awaited (or by the wake itself) and clears it; a sleep always clears the
//! wait and never readies; the resume deduper yields at most one wake between
//! sleeps; decoding never panics.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_platform::power::{GateAction, GateMachine, PowerEvent, ResumeDeduper};

fuzz_target!(|data: &[u8]| {
    let mut gate = GateMachine::new();
    let mut deduper = ResumeDeduper::default();
    let mut wakes_since_sleep = 0u32;
    for &byte in data.iter().take(4096) {
        let was_awaiting = gate.is_awaiting_network();
        let op = byte % 6;
        let actions = match op {
            0 => gate.will_sleep(),
            1 => gate.did_wake(),
            2 => gate.path_changed(true),
            3 => gate.path_changed(false),
            4 => gate.fallback_fired(),
            _ => {
                match deduper.accept(u32::from(byte >> 3)) {
                    Some(PowerEvent::WillSleep) => wakes_since_sleep = 0,
                    Some(PowerEvent::DidWake) => {
                        wakes_since_sleep += 1;
                        assert!(wakes_since_sleep <= 1);
                    }
                    None => {}
                }
                continue;
            }
        };
        let ready = actions.contains(&GateAction::Ready);
        if op == 0 {
            assert!(!gate.is_awaiting_network());
            assert!(!ready);
        }
        if ready {
            assert!(!gate.is_awaiting_network());
            assert!(was_awaiting || op == 1);
        }
    }
});
