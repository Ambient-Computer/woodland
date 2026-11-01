use std::collections::BTreeSet;

use signature::{Keypair, Signer, Verifier};

use crate::content_hash::{PayloadType, hasher_for, update_key, update_keys};
use crate::envelope::Payload;

/// Creates a new quorum. The quorum determines who has write access. All
/// mutable data must be associated with exactly one quorum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Initialize<PublicKey> {
    /// Who created the quorum. One controller may create many quorums, even
    /// with the same contributors; the nonce keeps them apart.
    pub controller: PublicKey,

    /// Random bytes that make the quorum unique. Without them, two quorums
    /// with the same controller and contributors would share an ID, and
    /// updates meant for one would merge into the other, permanently.
    pub nonce: [u8; 16],

    /// Initial actors with write access. Must not be empty.
    pub contributors: BTreeSet<PublicKey>,
}

impl<PublicKey: AsRef<[u8]>> Initialize<PublicKey> {
    /// Identifies the quorum. Primarily useful as a stable cache key.
    ///
    /// Covers what the quorum is, not the controller's signature on it. Some
    /// signature schemes let anyone rewrite a valid signature into another
    /// valid one, which would give one quorum several IDs.
    #[must_use]
    pub fn id(&self) -> blake3::Hash {
        let mut hasher = hasher_for(PayloadType::Initialize);
        update_key(&mut hasher, &self.controller);
        hasher.update(&self.nonce);
        update_keys(&mut hasher, &self.contributors);
        hasher.finalize()
    }
}

impl<PublicKey: AsRef<[u8]>> Payload for Initialize<PublicKey> {
    fn digest(&self) -> blake3::Hash {
        self.id()
    }
}

/// A quorum's initial state, signed by its controller.
///
/// The signature proves the controller's key can sign, so a broken key is
/// caught at creation rather than when it's needed. It also proves who
/// created the quorum, so nobody can create one in the controller's name.
///
/// Untrusted until verified, so the initial state is only reachable through
/// [`verify`](Self::verify).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Genesis<PublicKey, Signature> {
    pub(super) initial_state: Initialize<PublicKey>,
    pub signature: Signature,
}

impl<PublicKey: AsRef<[u8]>, Signature> Genesis<PublicKey, Signature> {
    /// Creates a quorum controlled by the signer's public key.
    ///
    /// The controller signs the quorum's ID. Unlike an envelope, there's no
    /// quorum to bind it to: the ID is the quorum.
    ///
    /// # Errors
    ///
    /// Fails if the signer does, e.g. a hardware token that is unavailable.
    pub fn sign(
        controller: &(impl Signer<Signature> + Keypair<VerifyingKey = PublicKey>),
        nonce: [u8; 16],
        contributors: BTreeSet<PublicKey>,
    ) -> Result<Self, signature::Error> {
        let initial_state = Initialize {
            controller: controller.verifying_key(),
            nonce,
            contributors,
        };

        let signature = controller.try_sign(initial_state.id().as_bytes())?;

        Ok(Self {
            initial_state,
            signature,
        })
    }

    /// Checks that the controller signed this initial state, then hands it
    /// over.
    ///
    /// # Errors
    ///
    /// Fails if the signature is invalid.
    pub fn verify(&self) -> Result<&Initialize<PublicKey>, signature::Error>
    where
        PublicKey: Verifier<Signature>,
    {
        self.initial_state
            .controller
            .verify(self.initial_state.id().as_bytes(), &self.signature)?;

        Ok(&self.initial_state)
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::Signature;

    use super::*;
    use crate::testing::{PublicKey, controller, keys, laptop, nas, phone};

    type Genesis = super::Genesis<PublicKey, Signature>;

    fn initialize() -> Initialize<PublicKey> {
        Initialize {
            controller: controller().key(),
            nonce: [0; 16],
            contributors: keys(&[&phone(), &laptop()]),
        }
    }

    #[test]
    fn id_is_stable() {
        assert_eq!(
            initialize().id().to_hex().as_str(),
            "f0941ad9b8e31a5285f68e102eee97ccae7d73c6eba65cebb8d2970cdde7fd00"
        );
    }

    #[test]
    fn id_depends_on_every_field() {
        let other_controller = Initialize {
            controller: phone().key(),
            ..initialize()
        };
        let other_nonce = Initialize {
            nonce: [1; 16],
            ..initialize()
        };
        let other_contributors = Initialize {
            contributors: keys(&[&phone(), &nas()]),
            ..initialize()
        };

        assert_ne!(initialize().id(), other_controller.id());
        assert_ne!(initialize().id(), other_nonce.id());
        assert_ne!(initialize().id(), other_contributors.id());
    }

    #[test]
    fn verifies_what_it_signs() -> Result<(), signature::Error> {
        let genesis = Genesis::sign(&controller(), [0; 16], keys(&[&phone(), &laptop()]))?;

        assert_eq!(genesis.verify()?, &initialize());
        Ok(())
    }

    #[test]
    fn rejects_a_tampered_initial_state() -> Result<(), signature::Error> {
        let mut genesis = Genesis::sign(&controller(), [0; 16], keys(&[&phone()]))?;
        genesis.initial_state.contributors = keys(&[&phone(), &nas()]);

        assert!(genesis.verify().is_err());
        Ok(())
    }

    #[test]
    fn rejects_an_imposter() -> Result<(), signature::Error> {
        let mut genesis = Genesis::sign(&phone(), [0; 16], keys(&[&phone()]))?;
        genesis.initial_state.controller = controller().key();

        assert!(genesis.verify().is_err());
        Ok(())
    }
}
