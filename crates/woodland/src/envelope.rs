use signature::{Keypair, Signer, Verifier};

/// Content an [`Envelope`] can carry.
pub trait Payload {
    /// A fingerprint of the value, signed in its place.
    ///
    /// Signing a fixed-size digest means signatures don't depend on a wire
    /// format, and the digest doubles as the value's ID. The cost: anything
    /// the digest leaves out isn't signed, so it must cover every field.
    fn digest(&self) -> blake3::Hash;
}

/// A payload signed by its author for one quorum.
///
/// The signature covers the quorum's ID, so a contributor using the same key
/// in several quorums can't have its payloads replayed from one to another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope<PublicKey, Signature, T> {
    pub author: PublicKey,
    pub payload: T,
    pub signature: Signature,
}

impl<PublicKey, Signature, T: Payload> Envelope<PublicKey, Signature, T> {
    /// Signs `payload` for `quorum` as the signer's public key.
    ///
    /// # Errors
    ///
    /// Fails if the signer does, e.g. a hardware token that is unavailable.
    pub fn sign(
        signer: &(impl Signer<Signature> + Keypair<VerifyingKey = PublicKey>),
        quorum: &blake3::Hash,
        payload: T,
    ) -> Result<Self, signature::Error> {
        let signature = signer.try_sign(&signed_bytes(quorum, &payload))?;

        Ok(Self {
            author: signer.verifying_key(),
            payload,
            signature,
        })
    }

    /// Checks that `author` signed this payload for `quorum`.
    ///
    /// # Errors
    ///
    /// Fails if the signature is invalid.
    pub fn verify(&self, quorum: &blake3::Hash) -> Result<(), signature::Error>
    where
        PublicKey: Verifier<Signature>,
    {
        self.author
            .verify(&signed_bytes(quorum, &self.payload), &self.signature)
    }
}

/// What the author actually signs: the quorum's ID, then the payload's digest.
///
/// One key may contribute to several quorums, but a payload only means
/// something in the quorum it was written for. Including the quorum's ID
/// keeps a signature from being replayed into another quorum.
fn signed_bytes(quorum: &blake3::Hash, payload: &impl Payload) -> [u8; 64] {
    let mut bytes = [0; 64];
    bytes[..32].copy_from_slice(quorum.as_bytes());
    bytes[32..].copy_from_slice(payload.digest().as_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signature, SigningKey, VerifyingKey};

    use super::*;
    use crate::quorum::{Change, Initialize};

    type Signed = Envelope<VerifyingKey, Signature, Change<&'static str>>;

    // The same key for the same device in every run.
    fn signing_key(device: &str) -> SigningKey {
        SigningKey::from_bytes(blake3::hash(device.as_bytes()).as_bytes())
    }

    fn quorum(contributors: &[&'static str]) -> blake3::Hash {
        Initialize {
            controller: "controller",
            contributors: contributors.iter().copied().collect(),
        }
        .id()
    }

    fn set_contributors(contributors: &[&'static str]) -> Change<&'static str> {
        Change::SetContributors {
            contributors: contributors.iter().copied().collect(),
        }
    }

    #[test]
    fn verifies_what_it_signs() -> Result<(), signature::Error> {
        let quorum = quorum(&["phone", "laptop"]);
        let envelope = Signed::sign(&signing_key("phone"), &quorum, set_contributors(&["phone"]))?;

        assert_eq!(envelope.author, signing_key("phone").verifying_key());
        envelope.verify(&quorum)
    }

    #[test]
    fn rejects_another_quorum() -> Result<(), signature::Error> {
        let laptop_quorum = quorum(&["phone", "laptop"]);
        let nas_quorum = quorum(&["phone", "nas"]);
        let envelope = Signed::sign(
            &signing_key("phone"),
            &laptop_quorum,
            set_contributors(&["phone"]),
        )?;

        assert!(envelope.verify(&nas_quorum).is_err());
        Ok(())
    }

    #[test]
    fn rejects_a_tampered_payload() -> Result<(), signature::Error> {
        let quorum = quorum(&["phone", "laptop"]);
        let mut envelope =
            Signed::sign(&signing_key("phone"), &quorum, set_contributors(&["phone"]))?;
        envelope.payload = set_contributors(&["phone", "nas"]);

        assert!(envelope.verify(&quorum).is_err());
        Ok(())
    }

    #[test]
    fn rejects_another_author() -> Result<(), signature::Error> {
        let quorum = quorum(&["phone", "laptop"]);
        let mut envelope =
            Signed::sign(&signing_key("phone"), &quorum, set_contributors(&["phone"]))?;
        envelope.author = signing_key("laptop").verifying_key();

        assert!(envelope.verify(&quorum).is_err());
        Ok(())
    }
}
