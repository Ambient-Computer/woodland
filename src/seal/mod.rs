//! Sealing: the keys and ciphers behind a group only its members read.
//!
//! A sealed group has a secret. Everything its contributors write is
//! encrypted under it, and it's handed to each peer that should hold
//! it by encrypting it to that peer's sealing key.
//!
//! Nothing here is random. The library never makes randomness, so
//! every key and nonce is derived: from a device's own secret key,
//! which nobody else can do, and from the thing being sealed, so that
//! two different things never share a nonce.

use std::fmt;

use ed25519_dalek::Signature;
use serde::{Deserialize, Serialize};
use x25519_dalek::PublicKey;

use crate::actor::{Identity, Peer};

mod keyring;
mod secret;

pub(crate) use keyring::{Keyring, Opened};
pub(crate) use secret::{Commitment, Grant, Secret};
#[cfg(test)]
mod tests;

const VOUCH: &[u8] = b"woodland/sealing key/vouch";

/// The key a device is sent secrets under: what another device needs
/// in order to let this one read a sealed group.
///
/// It's separate from the key the device signs with, and signed by it,
/// so nobody can pass off a key of their own as someone else's. Get
/// this device's from [`Database::sealing_key`], and hand it to
/// whoever gives out roles.
///
/// [`Database::sealing_key`]: crate::Database::sealing_key
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SealingKey {
    pub(super) key: [u8; 32],
    /// The owner's signature over the key.
    vouch: Vec<u8>,
}

impl SealingKey {
    pub(crate) fn of(identity: &Identity) -> Self {
        let key = PublicKey::from(&identity.sealing()).to_bytes();
        let vouch = identity.sign(&[VOUCH, &key].concat());

        Self {
            key,
            vouch: vouch.to_bytes().to_vec(),
        }
    }

    /// The key and its owner's signature for it, as bytes to keep or
    /// send. Neither is secret.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        [&self.key[..], &self.vouch].concat()
    }

    /// The key that [`to_bytes`] gave these for. `None` if they're too
    /// short to be one. Whether its owner signed for it is checked
    /// when a role names it: a group never takes one that isn't.
    ///
    /// [`to_bytes`]: Self::to_bytes
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let (key, vouch) = bytes.split_first_chunk::<32>()?;

        Some(Self {
            key: *key,
            vouch: vouch.to_vec(),
        })
    }

    /// Whether `peer` signed for this key.
    pub(crate) fn is_vouched_by(&self, peer: Peer) -> bool {
        let Ok(vouch) = Signature::from_slice(&self.vouch) else {
            return false;
        };
        let message = [VOUCH, &self.key].concat();

        peer.id().verify_strict(&message, &vouch).is_ok()
    }
}

impl fmt::Debug for SealingKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let short = u32::from_be_bytes([self.key[0], self.key[1], self.key[2], self.key[3]]);

        write!(formatter, "SealingKey({short:08x})")
    }
}
