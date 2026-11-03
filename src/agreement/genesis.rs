use std::collections::BTreeSet;
use std::fmt;

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

use super::Configuration;
use crate::actor::Peer;
use crate::encoding;

/// How an agreement began: who founded it, plus a nonce.
///
/// The genesis is stored with the agreement's data and is never modified.
/// Its hash is the agreement's [`AgreementId`], so any device holding the
/// genesis can check that the ID it was given is genuine, and two agreements
/// with the same founders still get different IDs.
///
/// The application supplies the nonce. The library never generates
/// randomness, which keeps it deterministic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Genesis {
    nonce: [u8; 32],
    founders: BTreeSet<Peer>,
}

/// Names an agreement. The BLAKE3 hash of its [`Genesis`].
///
/// Peers use it to say which agreements they're subscribed to, and storage
/// uses it to scope every key the agreement owns.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AgreementId([u8; 32]);

impl Genesis {
    /// Founders are the first configuration: the only contributors until the
    /// agreement learns a reconfiguration.
    pub fn new(nonce: [u8; 32], founders: impl IntoIterator<Item = VerifyingKey>) -> Self {
        Self {
            nonce,
            founders: founders.into_iter().map(Peer::from).collect(),
        }
    }

    #[must_use]
    pub fn id(&self) -> AgreementId {
        AgreementId(encoding::hash("woodland/agreement/genesis", self))
    }

    /// The encoded form, for handing to an invited device out of band.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        encoding::encode(self)
    }

    /// Reads a genesis created by [`Genesis::to_bytes`].
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        encoding::decode(bytes)
    }

    /// The configuration every replica starts from.
    pub(crate) fn configuration(&self) -> Configuration {
        Configuration::founded_by(self.founders.iter().copied())
    }
}

impl AgreementId {
    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

/// Abbreviated, like a git hash. The full ID is noise in logs.
impl fmt::Debug for AgreementId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..4] {
            write!(formatter, "{byte:02x}")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::peers::{laptop, phone};

    fn genesis(nonce: u8) -> Genesis {
        Genesis::new([nonce; 32], [phone().id(), laptop().id()])
    }

    #[test]
    fn id_is_stable() {
        assert_eq!(genesis(1).id(), genesis(1).id());
    }

    #[test]
    fn nonce_distinguishes_agreements_with_the_same_founders() {
        assert_ne!(genesis(1).id(), genesis(2).id());
    }

    #[test]
    fn survives_the_round_trip() {
        let bytes = genesis(1).to_bytes();

        assert_eq!(Genesis::from_bytes(&bytes), Some(genesis(1)));
    }
}
