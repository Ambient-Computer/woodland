//! Blobs: bytes too large or too opaque to be a value.
//!
//! A transaction writes a [`BlobRef`], which is an ordinary value. It
//! says what the bytes are and never that anyone has them: the bytes
//! are kept and moved apart from history, by whichever devices choose
//! to hold them. `docs/spec/blobs.md` is the design.
//!
//! This much is part of every build, because every device has to tell
//! a reference when it sees one: to carry the list an entry comes
//! with, and to work out what a checkpoint commits to.

#[cfg(feature = "blobs")]
pub(crate) mod fetch;
#[cfg(feature = "blobs")]
pub(crate) mod hash;
#[cfg(feature = "blobs")]
pub(crate) mod kept;
pub(crate) mod keys;
pub(crate) mod tally;
#[cfg(feature = "blobs")]
pub(crate) mod want;

use std::fmt;

use serde::{Deserialize, Serialize};

/// What a reference names. A blob is the only kind there is. Another
/// would be an addition to the vocabulary, and a device that predates
/// it treats the operation as one it doesn't know.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
enum Names {
    Blob,
}

/// A reference to a blob: its name, how long it is, and in a sealed
/// group the key it's encrypted under.
///
/// The name is the BLAKE3 hash of the bytes as they're kept, which in
/// a sealed group is encrypted. A blob is its name and its length
/// together: a reference with the wrong length is to a blob that
/// doesn't exist.
///
/// It describes nothing of what the blob contains. A file's name and
/// type are columns beside it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BlobRef {
    names: Names,
    name: [u8; 32],
    length: u64,
    key: Option<[u8; 32]>,
}

/// A blob as every device can see it, readable or not: its name and
/// its length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct Listed {
    pub(crate) name: [u8; 32],
    pub(crate) length: u64,
}

impl BlobRef {
    #[cfg_attr(
        not(any(test, feature = "blobs")),
        expect(dead_code, reason = "made by imports, which need the feature")
    )]
    pub(crate) fn new(name: [u8; 32], length: u64, key: Option<[u8; 32]>) -> Self {
        Self {
            names: Names::Blob,
            name,
            length,
            key,
        }
    }

    /// The BLAKE3 hash of the blob's bytes, as they're kept.
    #[must_use]
    pub fn name(&self) -> [u8; 32] {
        self.name
    }

    /// How many bytes the blob is.
    #[must_use]
    pub fn length(&self) -> u64 {
        self.length
    }

    /// Whether it's for a sealed group: its bytes are encrypted, and
    /// it carries their key.
    #[must_use]
    pub fn is_sealed(&self) -> bool {
        self.key.is_some()
    }

    pub(crate) fn listed(&self) -> Listed {
        Listed {
            name: self.name,
            length: self.length,
        }
    }
}

/// The key is left out: what's printed ends up in logs.
impl fmt::Debug for BlobRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "BlobRef(")?;
        for byte in &self.name[..8] {
            write!(formatter, "{byte:02x}")?;
        }

        write!(formatter, ", {} bytes", self.length)?;
        if self.is_sealed() {
            write!(formatter, ", sealed")?;
        }

        write!(formatter, ")")
    }
}
