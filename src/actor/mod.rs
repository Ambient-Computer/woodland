use ed25519_dalek::VerifyingKey;

/// Another device, identified by its Ed25519 public key (the same as an Iroh
/// node ID).
///
/// A peer may be an observer, not only a contributor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Peer(VerifyingKey);

impl From<VerifyingKey> for Peer {
    fn from(key: VerifyingKey) -> Self {
        Self(key)
    }
}
