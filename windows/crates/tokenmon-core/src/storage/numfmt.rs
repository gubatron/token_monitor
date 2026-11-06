//! JSON number encoding that matches Swift's `JSONEncoder` for whole numbers.

use serde::{Deserialize, Serialize, Serializer};

/// An `f64` that serializes whole values as JSON integers (`7`, not `7.0`).
///
/// Swift's `JSONEncoder` writes `7` for `7.0`; keeping that makes the files
/// textually close to the macOS app's and keeps `"2026-09-14":7` greppable.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(transparent)]
pub struct Num(pub f64);

impl Serialize for Num {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let v = self.0;
        if v.is_finite() && v.fract() == 0.0 && v.abs() < 9.0e15 {
            serializer.serialize_i64(v as i64)
        } else {
            serializer.serialize_f64(v)
        }
    }
}
