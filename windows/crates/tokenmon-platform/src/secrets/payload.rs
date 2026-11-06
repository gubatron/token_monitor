//! The decrypted vault JSON: `{"entries":{"<prefix><key>":"<secret>",...}}`.
//!
//! Spec 1.6.3 also carries `checkedSeparateItems`; Windows has no legacy
//! per-account items to adopt, so the field is accepted and ignored on read
//! and omitted on write.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Deserialize;
use thiserror::Error;
use zeroize::Zeroizing;

/// Largest vault or pending-record plaintext accepted (hostile-file bound).
pub const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;

/// A vault payload could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PayloadError {
    /// Larger than [`MAX_PAYLOAD_BYTES`].
    #[error("payload exceeds {MAX_PAYLOAD_BYTES} bytes")]
    TooLarge,
    /// Not the expected JSON. Deliberately carries no content: it may be secret.
    #[error("payload is not valid vault JSON")]
    Malformed,
}

/// Decoded vault contents. Values are wiped on drop and never printed.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct VaultPayload {
    /// `<prefix><key>` to secret.
    pub entries: BTreeMap<String, Zeroizing<String>>,
}

impl fmt::Debug for VaultPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VaultPayload")
            .field("entries", &self.entries.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Deserialize)]
struct Wire {
    entries: BTreeMap<String, String>,
}

/// Decodes untrusted decrypted bytes. Never panics.
pub fn decode_payload(bytes: &[u8]) -> Result<VaultPayload, PayloadError> {
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(PayloadError::TooLarge);
    }
    let wire: Wire = serde_json::from_slice(bytes).map_err(|_| PayloadError::Malformed)?;
    Ok(VaultPayload {
        entries: wire
            .entries
            .into_iter()
            .map(|(k, v)| (k, Zeroizing::new(v)))
            .collect(),
    })
}

/// Encodes the payload to plaintext JSON, wiped on drop.
pub fn encode_payload(payload: &VaultPayload) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(Vec::with_capacity(64));
    out.extend_from_slice(b"{\"entries\":{");
    for (i, (k, v)) in payload.entries.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        push_json_string(&mut out, k);
        out.push(b':');
        push_json_string(&mut out, v);
    }
    out.extend_from_slice(b"}}");
    out
}

/// Appends `s` as a JSON string straight into the wiped buffer, so no
/// unwiped intermediate copy of a secret exists.
fn push_json_string(out: &mut Vec<u8>, s: &str) {
    out.push(b'"');
    for ch in s.chars() {
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            c if (c as u32) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes());
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

/// Decodes the pending-removals record (a JSON string array); duplicates
/// collapse. Never panics.
pub fn decode_pending(bytes: &[u8]) -> Result<BTreeSet<String>, PayloadError> {
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(PayloadError::TooLarge);
    }
    serde_json::from_slice::<Vec<String>>(bytes)
        .map(|v| v.into_iter().collect())
        .map_err(|_| PayloadError::Malformed)
}
