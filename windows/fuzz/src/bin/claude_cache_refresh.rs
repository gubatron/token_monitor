#![no_main]
//! Random append / partial-append / rewrite / delete sequences on transcript
//! files: the incremental cache must agree with a from-scratch scan. Lines
//! carry no dedupe key, so arrival order cannot change totals. mtimes are set
//! explicitly so a same-size rewrite is always visible.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::time::{Duration, SystemTime};

use libfuzzer_sys::fuzz_target;
use tokenmon_provider_claude::{ClaudeCodeFileCache, DayCalendar};

fn line(n: u8, crlf: bool) -> String {
    let eol = if crlf { "\r\n" } else { "\n" };
    format!(
        "{{\"type\":\"assistant\",\"timestamp\":\"2026-10-1{}T10:00:00Z\",\"message\":{{\"model\":\"m{}\",\"usage\":{{\"input_tokens\":{}}}}}}}{eol}",
        n % 3,
        n % 2,
        n
    )
}

fuzz_target!(|data: &[u8]| {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = ClaudeCodeFileCache::new();
    let mut tick = 0u64;
    for op in data.chunks(3).take(24) {
        let kind = op[0];
        let file = op.get(1).copied().unwrap_or(0);
        let arg = op.get(2).copied().unwrap_or(0);
        let path = dir.path().join(format!("f{}.jsonl", file % 3));
        let text = line(arg, arg % 5 == 0);
        match kind % 5 {
            0 | 1 => {
                if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
                    let _ = f.write_all(text.as_bytes());
                }
            }
            2 => {
                let cut = text.len() / 2;
                if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
                    let _ = f.write_all(&text.as_bytes()[..cut]);
                }
            }
            3 => {
                let _ = fs::write(&path, text.as_bytes());
            }
            _ => {
                let _ = fs::remove_file(&path);
            }
        }
        tick += 1;
        if let Ok(f) = File::options().write(true).open(&path) {
            let _ = f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + tick));
        }
        cache.update(dir.path());
        let incremental = cache.daily_tokens(DayCalendar::Utc);
        let fresh = ClaudeCodeFileCache::new();
        fresh.update(dir.path());
        assert_eq!(incremental, fresh.daily_tokens(DayCalendar::Utc));
    }
});
