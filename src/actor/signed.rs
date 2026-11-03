//! Signatures over payloads, and the batch verification that gates remote
//! data before it's integrated.
//!
//! The type split is the point: [`Signed`] is what travels and what's stored.
//! [`Verified`] can only be made by checking the signature, so code that
//! integrates remote data takes `Verified<T>` and can't skip the check.

use ed25519_dalek::{Signature, Verifier, VerifyingKey, verify_batch};
use serde::{Deserialize, Serialize};

use super::{Identity, Peer};
use crate::encoding;

/// A payload type that can be signed.
///
/// Each type signs under its own context string, so a signature over one
/// kind of payload can never be replayed as another kind with the same bytes
/// (domain separation).
pub(crate) trait Signable: Serialize {
    const CONTEXT: &'static [u8];
}

/// A payload with its author's signature. Not yet checked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Signed<T> {
    author: Peer,
    signature: Signature,
    payload: T,
}

/// A payload whose signature has been checked against its author.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Verified<T>(Signed<T>);

impl<T: Signable> Signed<T> {
    /// Every local operation is signed as it's created, so the same bytes can
    /// be stored, relayed by anyone, and verified by everyone.
    pub(crate) fn sign(identity: &Identity, payload: T) -> Verified<T> {
        let signature = identity.sign(&signed_bytes(&payload));

        Verified(Self {
            author: identity.peer(),
            signature,
            payload,
        })
    }

    pub(crate) fn author(&self) -> Peer {
        self.author
    }

    pub(crate) fn payload(&self) -> &T {
        &self.payload
    }

    fn verify(self) -> Option<Verified<T>> {
        let key: VerifyingKey = self.author.id();
        let valid = key
            .verify(&signed_bytes(&self.payload), &self.signature)
            .is_ok();

        valid.then_some(Verified(self))
    }
}

impl<T> Verified<T> {
    pub(crate) fn author(&self) -> Peer {
        self.0.author
    }

    pub(crate) fn payload(&self) -> &T {
        &self.0.payload
    }

    /// Back to the transportable form, for storing or relaying.
    pub(crate) fn signed(&self) -> &Signed<T> {
        &self.0
    }

    /// Trusts a signature checked earlier, such as data this device wrote to
    /// its own storage. Never use it for data that came over the network.
    pub(crate) fn trust_stored(signed: Signed<T>) -> Self {
        Self(signed)
    }
}

/// Verifies a batch of signatures at once, dropping any that fail.
///
/// Batch verification is much cheaper than one-by-one, but it only answers
/// "all valid" or "something is wrong". When something is wrong, it falls
/// back to checking each one, so a single forgery from a misbehaving peer
/// doesn't cost us the honest data in the same message.
pub(crate) fn verify_all<T: Signable>(batch: Vec<Signed<T>>) -> Vec<Verified<T>> {
    let messages: Vec<Vec<u8>> = batch.iter().map(|it| signed_bytes(&it.payload)).collect();
    let message_refs: Vec<&[u8]> = messages.iter().map(Vec::as_slice).collect();
    let signatures: Vec<Signature> = batch.iter().map(|it| it.signature).collect();
    let keys: Vec<VerifyingKey> = batch.iter().map(|it| it.author.id()).collect();

    if verify_batch(&message_refs, &signatures, &keys).is_ok() {
        return batch.into_iter().map(Verified).collect();
    }

    let total = batch.len();
    let valid: Vec<_> = batch.into_iter().filter_map(Signed::verify).collect();
    tracing::warn!(
        total,
        rejected = total - valid.len(),
        "dropped forged signatures"
    );

    valid
}

/// What actually gets signed: the context, then the canonical encoding.
fn signed_bytes<T: Signable>(payload: &T) -> Vec<u8> {
    let mut bytes = T::CONTEXT.to_vec();
    bytes.extend(encoding::encode(payload));

    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::peers::{laptop, laptop_identity, phone_identity};

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Note(String);

    impl Signable for Note {
        const CONTEXT: &'static [u8] = b"woodland/test/note";
    }

    fn note(identity: &Identity, text: &str) -> Signed<Note> {
        Signed::sign(identity, Note(text.into())).signed().clone()
    }

    #[test]
    fn accepts_honest_signatures() {
        let batch = vec![
            note(&phone_identity(), "hi"),
            note(&laptop_identity(), "hello"),
        ];

        assert_eq!(verify_all(batch).len(), 2);
    }

    #[test]
    fn drops_only_the_forgery() {
        let mut forged = note(&phone_identity(), "hi");
        forged.author = laptop();

        let verified = verify_all(vec![forged, note(&laptop_identity(), "hello")]);

        assert_eq!(verified.len(), 1);
        assert_eq!(verified[0].payload(), &Note("hello".into()));
    }

    #[test]
    fn detects_tampered_payloads() {
        let mut tampered = note(&phone_identity(), "hi");
        tampered.payload = Note("bye".into());

        assert_eq!(verify_all(vec![tampered]), []);
    }
}
