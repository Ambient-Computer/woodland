//! Who is speaking: this device ([`Identity`]), other devices ([`Peer`]), and
//! the signatures that tie data to its author ([`Signed`]).

mod identity;
mod signed;

use std::cmp::Ordering;
use std::fmt;

use ed25519_dalek::VerifyingKey;
pub use identity::Identity;
use serde::{Deserialize, Serialize};
pub(crate) use signed::{Signable, Signed, Verified, verify_all};

/// A device, identified by its Ed25519 public key (the same as an Iroh node
/// ID).
///
/// A peer may be an observer, not only a contributor. Whether it may write is
/// decided by an agreement's configuration, not by this type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Peer(VerifyingKey);

/// A short key prefix, or the device's name in tests. Full keys make
/// protocol traces and proptest counterexamples unreadable.
impl fmt::Debug for Peer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(test)]
        if let Some(name) = crate::testing::peers::name(*self) {
            return formatter.write_str(name);
        }

        for byte in &self.as_bytes()[..4] {
            write!(formatter, "{byte:02x}")?;
        }

        Ok(())
    }
}

impl From<VerifyingKey> for Peer {
    fn from(key: VerifyingKey) -> Self {
        Self(key)
    }
}

impl Peer {
    /// Peers are identified by their public signing key.
    pub(crate) fn id(&self) -> VerifyingKey {
        self.0
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// Reads a peer back from its key bytes. Not every 32 bytes are a key.
    pub(crate) fn from_bytes(bytes: &[u8; 32]) -> Option<Self> {
        VerifyingKey::from_bytes(bytes).ok().map(Self)
    }
}

/// Peers are ordered by their key bytes.
///
/// The order carries no meaning, but it must be the same on every device:
/// peers are keys in `BTreeMap`s that get hashed and signed, and they break
/// ties between concurrent writes.
impl Ord for Peer {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl PartialOrd for Peer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
