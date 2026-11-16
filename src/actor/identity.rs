use std::fmt;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use super::Peer;

/// "This is me": the signing keypair of the device running the [`Database`].
///
/// The library never generates keys. Randomness and secret storage belong
/// to the application, which keeps the library deterministic and testable.
///
/// [`Database`]: crate::Database
pub struct Identity {
    keypair: SigningKey,
}

impl Identity {
    /// Wraps an existing Ed25519 secret key.
    #[must_use]
    pub fn from_secret_key(secret: [u8; 32]) -> Self {
        Self {
            keypair: SigningKey::from_bytes(&secret),
        }
    }

    /// The public half, which is how other devices know this one.
    #[must_use]
    pub fn public_key(&self) -> VerifyingKey {
        self.keypair.verifying_key()
    }

    pub(crate) fn peer(&self) -> Peer {
        self.public_key().into()
    }

    pub(crate) fn sign(&self, message: &[u8]) -> Signature {
        self.keypair.sign(message)
    }

    /// The secret half of the key this device is sent secrets under.
    /// It's worked out from the signing key and can't be worked back,
    /// so the two are separate keys with one thing to keep safe.
    pub(crate) fn sealing(&self) -> x25519_dalek::StaticSecret {
        let seed = self.keypair.to_bytes();

        blake3::derive_key("woodland/sealing key", &seed).into()
    }
}

/// Prints only the public key. The secret must never reach logs.
impl fmt::Debug for Identity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Identity")
            .field("public_key", &self.public_key())
            .finish_non_exhaustive()
    }
}
