//! Signature policy selection and the Authenticode fail-closed behaviour.

use std::io::Write;

use tokenmon_platform::install::verify::{SignaturePolicy, verifier_for_policy};

fn fake_pe() -> tempfile::NamedTempFile {
    let mut bytes = vec![0u8; 128];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[0x3C] = 0x40;
    bytes[0x40..0x44].copy_from_slice(b"PE\0\0");
    let mut f = tempfile::NamedTempFile::new().expect("temp file");
    f.write_all(&bytes).expect("write");
    f
}

/// The default policy accepts an unsigned but structurally valid PE.
#[test]
fn test_default_policy_accepts_unsigned_pe() {
    assert_eq!(
        SignaturePolicy::default(),
        SignaturePolicy::AcceptUnsignedWithDigest
    );
    let v = verifier_for_policy(SignaturePolicy::default());
    assert!(v.verify_exe(fake_pe().path()).is_ok());
}

/// `RequireAuthenticode` refuses an unsigned PE (and everything off Windows).
#[test]
fn test_require_authenticode_refuses_unsigned_pe() {
    let v = verifier_for_policy(SignaturePolicy::RequireAuthenticode);
    assert!(v.verify_exe(fake_pe().path()).is_err());
}

/// Both policies reject a file that is not a PE.
#[test]
fn test_both_policies_reject_non_pe() {
    let mut f = tempfile::NamedTempFile::new().expect("temp file");
    f.write_all(b"hello").expect("write");
    for p in [
        SignaturePolicy::AcceptUnsignedWithDigest,
        SignaturePolicy::RequireAuthenticode,
    ] {
        assert!(verifier_for_policy(p).verify_exe(f.path()).is_err());
    }
}
