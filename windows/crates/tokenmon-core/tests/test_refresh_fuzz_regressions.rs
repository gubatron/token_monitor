//! Stable-toolchain replay of the `core_cookie_merge` and `net_domain_matches`
//! fuzz targets: every corpus seed plus a deterministic pseudo-random sweep must
//! satisfy the targets' invariants.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use chrono::{TimeZone, Utc};
use tokenmon_core::AuthConfig;
use tokenmon_core::auth_session::{merge_refreshed_cookies, parse_refreshed_cookies};
use tokenmon_core::net::Domain;

fn corpus(target: &str) -> Vec<Vec<u8>> {
    let dir: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/corpus")
        .join(target);
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| std::fs::read(e.path()).unwrap())
        .collect()
}

/// xorshift64: deterministic, dependency-free.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[(self.next() % items.len() as u64) as usize]
    }
}

fn config(with_allowlist: bool) -> AuthConfig {
    AuthConfig {
        store_filename_prefix: "t_".into(),
        log_category: "Fuzz".into(),
        hosts: vec![],
        sign_out_hosts: vec!["example.com".into()],
        extra_store_keys: vec![],
        preferred_session_cookies: vec!["sso".into()],
        essential_cookie_names: if with_allowlist {
            vec!["sso".into()]
        } else {
            vec![]
        },
        essential_cookie_prefixes: if with_allowlist {
            vec!["__secure-next-auth.session-token".into()]
        } else {
            vec![]
        },
        account_identity_cookie: None,
        max_attempts: 1,
        retry_delay: AuthConfig::DEFAULT_RETRY_DELAY,
        failure_message: String::new(),
    }
}

fn unsafe_byte(c: char) -> bool {
    matches!(c, '\r' | '\n' | '\0')
}

/// The `core_cookie_merge` invariants (kept in step with the fuzz target).
fn check_merge(data: &[u8]) {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let cfg = config(mode & 1 == 1);
    let text = String::from_utf8_lossy(rest);
    let mut fields = text.split('\u{1f}');
    let stored = fields.next().unwrap_or("");
    let headers: Vec<String> = fields.map(str::to_owned).collect();
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let out = merge_refreshed_cookies(&cfg, stored, &headers, now);

    let clean_in =
        !stored.chars().any(unsafe_byte) && !headers.iter().any(|h| h.chars().any(unsafe_byte));
    if clean_in {
        assert!(!out.chars().any(unsafe_byte), "{text:?} -> {out:?}");
    }
    for piece in out.split("; ").filter(|p| !p.is_empty()) {
        assert!(piece.contains('='), "piece without '=': {piece:?}");
    }
    let stored_names: Vec<String> = stored
        .split(';')
        .map(|p| p.trim().split('=').next().unwrap_or("").to_lowercase())
        .collect();
    let refreshed = parse_refreshed_cookies(&headers, now);
    for piece in out.split("; ").filter(|p| !p.is_empty()) {
        let name = piece.split('=').next().unwrap_or("").trim().to_lowercase();
        if !stored_names.contains(&name) {
            assert!(cfg.is_essential(&name), "{text:?} -> {out:?}");
            assert!(
                refreshed
                    .iter()
                    .any(|c| c.live && c.name.to_lowercase() == name)
            );
        }
    }
    assert_eq!(
        merge_refreshed_cookies(&cfg, &out, &headers, now),
        out,
        "not idempotent for {text:?}"
    );
}

#[test]
fn test_cookie_merge_corpus_seeds_hold_the_invariants() {
    let seeds = corpus("core_cookie_merge");
    assert!(seeds.len() >= 5);
    for seed in seeds {
        check_merge(&seed);
    }
}

#[test]
fn test_cookie_merge_random_sweep_holds_the_invariants() {
    let names = [
        "sso",
        "SSO",
        "__Secure-next-auth.session-token",
        "__Secure-next-auth.session-token.0",
        "__Secure-next-auth.session-token.1",
        "_ga",
        "a",
        "",
        " ",
    ];
    let attrs = [
        "",
        "; Path=/",
        "; Max-Age=0",
        "; Max-Age=60",
        "; Max-Age=x",
        "; Expires=Wed, 21 Oct 2099 07:28:00 GMT",
        "; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
        "; Secure; HttpOnly",
    ];
    let values = ["", "v", "w=x", "a b", "q\"q", "\u{e9}"];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..20_000 {
        let mut input = vec![u8::try_from(rng.next() & 1).unwrap()];
        let stored_n = rng.next() % 4;
        let stored: Vec<String> = (0..stored_n)
            .map(|_| format!("{}={}", rng.pick(&names), rng.pick(&values)))
            .collect();
        input.extend(stored.join("; ").bytes());
        for _ in 0..(rng.next() % 4) {
            input.push(0x1F);
            let folded: Vec<String> = (0..=rng.next() % 2)
                .map(|_| {
                    format!(
                        "{}={}{}",
                        rng.pick(&names),
                        rng.pick(&values),
                        rng.pick(&attrs)
                    )
                })
                .collect();
            input.extend(folded.join(", ").bytes());
        }
        check_merge(&input);
    }
}

const HOSTS: [&str; 3] = ["claude.ai", "chatgpt.com", "x.ai"];

fn check_domain(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let lowered = text.to_lowercase();
    let d = lowered.trim_matches('.');
    let expected = HOSTS
        .iter()
        .any(|h| d == *h || d.ends_with(&format!(".{h}")));
    assert_eq!(Domain::matches(&text, &HOSTS), expected, "{text:?}");
    for h in HOSTS {
        assert!(!Domain::matches(&format!("not{h}"), &HOSTS));
        assert!(!Domain::matches(&format!("{h}.evil.com"), &HOSTS));
        assert!(Domain::matches(&format!("www.{h}"), &HOSTS));
    }
}

#[test]
fn test_domain_corpus_and_sweep_hold_the_invariants() {
    let seeds = corpus("net_domain_matches");
    assert!(seeds.len() >= 4);
    for seed in seeds {
        check_domain(&seed);
    }
    let parts = [
        "claude.ai",
        "x.ai",
        ".",
        "..",
        "evil",
        "not",
        "CLAUDE.AI",
        "-",
        "\u{130}",
    ];
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for _ in 0..20_000 {
        let n = 1 + rng.next() % 4;
        let s: String = (0..n).map(|_| rng.pick(&parts)).collect();
        check_domain(s.as_bytes());
    }
}
