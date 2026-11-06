#![allow(clippy::unwrap_used)]
//! Stable-toolchain replay of the `platform_vault_decode` fuzz corpus plus
//! hostile inputs: the decoders must never panic, and large vaults work.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use tempfile::TempDir;
use tokenmon_platform::secrets::{
    FakeEncryptor, Vault, decode_payload, decode_pending, encode_payload,
};

fn exercise(data: &[u8]) {
    if let Ok(p) = decode_payload(data) {
        assert_eq!(decode_payload(&encode_payload(&p)).unwrap(), p);
    }
    let _ = decode_pending(data);
}

#[test]
fn corpus_replays_without_panic() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/platform_vault_decode");
    let mut n = 0;
    for entry in fs::read_dir(dir).unwrap() {
        exercise(&fs::read(entry.unwrap().path()).unwrap());
        n += 1;
    }
    assert!(n >= 2);
}

#[test]
fn hostile_inputs_do_not_panic() {
    let deep_open = "[".repeat(100_000);
    let deep_obj = format!("{}1{}", "{\"entries\":".repeat(5000), "}".repeat(5000));
    for input in [
        &b""[..],
        b"\0\0\0",
        b"\xff\xfe\xfd",
        b"{\"entries\":",
        b"{\"entries\":{\"a\":\"\\ud800\"}}",
        b"{\"entries\":{\"a\":\"\\u0000\"}}",
        b"{\"entries\":null}",
        deep_open.as_bytes(),
        deep_obj.as_bytes(),
    ] {
        exercise(input);
    }
}

#[test]
fn vault_larger_than_credential_manager_limit_round_trips() {
    // Credential Manager caps a blob at 2,560 bytes; a chunked ChatGPT NextAuth
    // cookie header alone is bigger. The vault file has no such limit.
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    let big = "chunk=".to_owned() + &"x".repeat(12_000);
    assert!(Vault::new(dir.path(), enc.clone()).set(&big, "chatgpt_auth_session"));
    assert!(Vault::new(dir.path(), enc.clone()).set("k", "openrouter_auth_key"));
    let v = Vault::new(dir.path(), enc);
    assert_eq!(
        v.value("chatgpt_auth_session").as_deref(),
        Some(big.as_str())
    );
    assert_eq!(v.value("openrouter_auth_key").as_deref(), Some("k"));
}
