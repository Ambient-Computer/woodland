//! Hashes values by their content.
//!
//! Authors sign a value's digest, not the value itself. The application
//! owns the wire format, so signing the value would tie every signature to
//! one serialization; a digest is the same no matter how the value was
//! sent. Digests also double as IDs (e.g. a quorum's ID).
//!
//! A digest only stands in for a value if no other value encodes to the
//! same bytes. Otherwise a signature on one value would also verify for the
//! other. Two rules keep encodings unique:
//!
//! - Every encoding starts with its [`PayloadType`], so values of different
//!   types never match.
//! - Variable-length fields are prefixed with their length, so the same
//!   bytes can't be split into different fields (`["ab", "c"]` vs.
//!   `["a", "bc"]`).

use std::collections::BTreeSet;

/// Identifies what kind of value an encoding holds. The numbers are part of
/// every digest, so changing one changes every ID and breaks every existing
/// signature.
#[derive(Clone, Copy)]
#[repr(u8)]
pub(crate) enum PayloadType {
    Initialize = 0,
    Operation = 1,
    VersionVector = 2,
}

/// Starts an encoding for a value of the given type.
pub(crate) fn hasher_for(payload_type: PayloadType) -> blake3::Hasher {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&[payload_type as u8]);
    hasher
}

pub(crate) fn update_key(hasher: &mut blake3::Hasher, key: &impl AsRef<[u8]>) {
    let bytes = key.as_ref();
    update_len(hasher, bytes.len());
    hasher.update(bytes);
}

pub(crate) fn update_keys<PublicKey: AsRef<[u8]>>(
    hasher: &mut blake3::Hasher,
    keys: &BTreeSet<PublicKey>,
) {
    update_len(hasher, keys.len());
    for key in keys {
        update_key(hasher, key);
    }
}

pub(crate) fn update_len(hasher: &mut blake3::Hasher, len: usize) {
    hasher.update(&(len as u64).to_le_bytes());
}
