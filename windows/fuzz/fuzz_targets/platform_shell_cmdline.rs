//! Invariants: argument parsing and command-line splitting never panic;
//! lenient parsing never fails; a quotable path round-trips through
//! `build`/`executable`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_platform::shell::LaunchArgs;
use tokenmon_platform::shell::command_line::{build, executable};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = executable(&text);
    let _ = LaunchArgs::parse(text.split_whitespace());
    let _ = LaunchArgs::parse_lenient(text.split_whitespace());
    if let Ok(line) = build(&text, &["--show"]) {
        assert_eq!(executable(&line), Some(&*text));
    }
});
