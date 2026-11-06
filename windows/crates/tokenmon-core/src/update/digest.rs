//! Streaming SHA-256 with lowercase-hex output.

use sha2::{Digest, Sha256};

/// Incremental SHA-256 hasher whose result is lowercase hex.
///
/// # Examples
///
/// ```
/// use tokenmon_core::update::Sha256Stream;
///
/// let mut h = Sha256Stream::new();
/// h.update(b"a");
/// h.update(b"bc");
/// assert_eq!(
///     h.finish_hex(),
///     "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
/// );
/// ```
#[derive(Debug, Default)]
pub struct Sha256Stream(Sha256);

impl Sha256Stream {
    /// A fresh hasher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds another chunk.
    pub fn update(&mut self, chunk: &[u8]) {
        self.0.update(chunk);
    }

    /// Consumes the hasher and returns the 64-character lowercase hex digest.
    pub fn finish_hex(self) -> String {
        use std::fmt::Write;
        self.0
            .finalize()
            .iter()
            .fold(String::with_capacity(64), |mut out, b| {
                let _ = write!(out, "{b:02x}");
                out
            })
    }
}

/// One-shot SHA-256 of `bytes` as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256Stream::new();
    h.update(bytes);
    h.finish_hex()
}
