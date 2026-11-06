//! The PE `VS_VERSIONINFO` reader on arbitrary bytes: never panics, and a
//! version it returns re-parses from its own text.
#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use tokenmon_core::update::AppVersion;
use tokenmon_platform::install::read_pe_version;

fuzz_target!(|data: &[u8]| {
    if let Some(v) = read_pe_version(Cursor::new(data)) {
        let again = AppVersion::parse(&v.to_string()).expect("a read version re-parses");
        assert_eq!(again, v);
    }
});
