//! Real keys and quorums for tests, so tests follow the same paths an
//! application would.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use signature::{Keypair, Signer, Verifier};

use crate::quorum::{self, Initialize};

pub(crate) type Ledger = quorum::Ledger<PublicKey, Signature>;

/// An actor's public key.
///
/// The library needs keys to be `Ord`, which ed25519-dalek's key isn't, so
/// it's wrapped the way an application would wrap it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PublicKey(VerifyingKey);

impl Ord for PublicKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.as_bytes().cmp(other.0.as_bytes())
    }
}

impl PartialOrd for PublicKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl AsRef<[u8]> for PublicKey {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl Verifier<Signature> for PublicKey {
    fn verify(&self, message: &[u8], signature: &Signature) -> Result<(), signature::Error> {
        self.0.verify(message, signature)
    }
}

/// Something that signs: a device, or a role like a quorum's controller.
#[derive(Debug)]
pub(crate) struct Actor(SigningKey);

impl Actor {
    /// The same key for the same name in every run.
    fn named(name: &str) -> Self {
        Self(SigningKey::from_bytes(
            blake3::hash(name.as_bytes()).as_bytes(),
        ))
    }

    pub(crate) fn key(&self) -> PublicKey {
        self.verifying_key()
    }
}

impl Signer<Signature> for Actor {
    fn try_sign(&self, message: &[u8]) -> Result<Signature, signature::Error> {
        self.0.try_sign(message)
    }
}

impl Keypair for Actor {
    type VerifyingKey = PublicKey;

    fn verifying_key(&self) -> PublicKey {
        PublicKey(self.0.verifying_key())
    }
}

pub(crate) fn phone() -> Actor {
    Actor::named("phone")
}

pub(crate) fn laptop() -> Actor {
    Actor::named("laptop")
}

pub(crate) fn nas() -> Actor {
    Actor::named("nas")
}

/// A controller with a fixed key, for tests that need a stable quorum ID.
/// Real quorums get a fresh one; see [`quorum`].
pub(crate) fn controller() -> Actor {
    Actor::named("controller")
}

pub(crate) fn keys(actors: &[&Actor]) -> BTreeSet<PublicKey> {
    actors.iter().map(|actor| actor.key()).collect()
}

/// A new quorum of `contributors`, created the way an application would:
/// under a fresh controller key.
pub(crate) fn quorum(contributors: &[&Actor]) -> Ledger {
    // Unique per quorum, like a freshly generated key.
    static QUORUMS: AtomicU64 = AtomicU64::new(0);
    let count = QUORUMS.fetch_add(1, Ordering::Relaxed);
    let controller = Actor::named(&format!("controller {count}"));

    Ledger::new(Initialize {
        controller: controller.key(),
        contributors: keys(contributors),
    })
}
