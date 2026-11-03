//! The canonical byte encoding for everything that's sent, stored, signed or
//! hashed.
//!
//! Signatures and content hashes only work if every device produces the same
//! bytes for the same value. Postcard is deterministic for a given type, and
//! the types we encode avoid anything with unstable iteration order (no
//! `HashMap`, only `BTreeMap`/`BTreeSet`).

use serde::Serialize;
use serde::de::DeserializeOwned;

pub(crate) fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    // Encoding into a `Vec` can only fail for types serde can't represent,
    // which none of ours are.
    postcard::to_allocvec(value).unwrap_or_default()
}

/// Decodes untrusted bytes. Failure means the sender is buggy or hostile.
pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    postcard::from_bytes(bytes).ok()
}

/// A content hash over the canonical encoding, with a context string so
/// different kinds of value never share a hash.
pub(crate) fn hash<T: Serialize>(context: &str, value: &T) -> [u8; 32] {
    blake3::derive_key(context, &encode(value))
}
