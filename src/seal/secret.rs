//! A group's secret: what's encrypted under it, and how it's handed
//! to a peer.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "sealed groups are built on this next")
)]

use std::fmt;

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};

use super::SealingKey;
use crate::actor::Identity;

/// What a sealed group's contents are encrypted under.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Secret(pub(super) [u8; 32]);

/// Stands for a secret without giving it away. A checkpoint carries
/// one, so that a peer handed a secret can tell it's the right one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Commitment([u8; 32]);

/// A secret, encrypted to one peer's sealing key.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Grant {
    /// The public half of a key made for this grant alone.
    ephemeral: [u8; 32],
    sealed: Vec<u8>,
}

/// Never prints the secret.
impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Secret({:?})", self.commitment())
    }
}

fn derive(context: &str, parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(context);
    for part in parts {
        let length = u64::try_from(part.len()).unwrap_or(u64::MAX);
        hasher.update(&length.to_le_bytes());
        hasher.update(part);
    }

    *hasher.finalize().as_bytes()
}

fn encrypt(key: &[u8; 32], nonce: &[u8; 24], about: &[u8], plain: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let payload = Payload {
        msg: plain,
        aad: about,
    };

    // Fails only for messages too long to exist.
    cipher
        .encrypt(&XNonce::from(*nonce), payload)
        .unwrap_or_default()
}

fn decrypt(key: &[u8; 32], nonce: &[u8; 24], about: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let payload = Payload {
        msg: sealed,
        aad: about,
    };

    cipher.decrypt(&XNonce::from(*nonce), payload).ok()
}

impl Secret {
    /// A secret only `identity` could have made, for `purpose`. The
    /// same device asked twice for the same purpose makes the same one.
    pub(crate) fn make(identity: &Identity, purpose: &[u8]) -> Self {
        let seed = identity.sealing().to_bytes();

        Self(derive("woodland/secret", &[&seed, purpose]))
    }

    pub(crate) fn commitment(&self) -> Commitment {
        Commitment(derive("woodland/secret/commitment", &[&self.0]))
    }

    /// Encrypts `plain`. `about` says what it is and where it goes, and
    /// the result opens only when asked about the same thing.
    ///
    /// The nonce comes from the contents, so it travels with them, and
    /// sealing the same thing twice gives the same bytes: an author
    /// who signs two entries for one place can't make a nonce repeat
    /// over different contents.
    pub(crate) fn seal(&self, about: &[u8], plain: &[u8]) -> Vec<u8> {
        let key = derive("woodland/seal/key", &[&self.0]);
        let nonce = derive("woodland/seal/nonce", &[&self.0, about, plain]);
        let nonce: [u8; 24] = nonce.first_chunk().copied().unwrap_or_default();
        let sealed = encrypt(&key, &nonce, about, plain);

        [&nonce[..], &sealed].concat()
    }

    /// Decrypts what [`seal`] made, if this is the secret it was made
    /// under and `about` is what it was made about.
    ///
    /// [`seal`]: Self::seal
    pub(crate) fn open(&self, about: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
        let (nonce, sealed) = sealed.split_first_chunk::<24>()?;
        let key = derive("woodland/seal/key", &[&self.0]);

        decrypt(&key, nonce, about, sealed)
    }

    /// Encrypts the secret to `key`, so that only its owner reads it.
    /// `about` ties the grant to where it's used.
    pub(crate) fn grant(&self, from: &Identity, to: &SealingKey, about: &[u8]) -> Grant {
        let seed = from.sealing().to_bytes();
        let ephemeral = derive(
            "woodland/grant/ephemeral",
            &[&seed, &to.key, about, &self.0],
        );
        let ephemeral = StaticSecret::from(ephemeral);
        let public = PublicKey::from(&ephemeral).to_bytes();
        let shared = ephemeral.diffie_hellman(&PublicKey::from(to.key));
        let key = derive("woodland/grant/key", &[shared.as_bytes(), &public, &to.key]);

        Grant {
            ephemeral: public,
            sealed: encrypt(&key, &[0; 24], about, &self.0),
        }
    }
}

impl Grant {
    /// The secret, if this was made for `identity` and about `about`.
    pub(crate) fn open(&self, identity: &Identity, about: &[u8]) -> Option<Secret> {
        let mine = identity.sealing();
        let public = PublicKey::from(&mine).to_bytes();
        let shared = mine.diffie_hellman(&PublicKey::from(self.ephemeral));
        let key = derive(
            "woodland/grant/key",
            &[shared.as_bytes(), &self.ephemeral, &public],
        );
        let secret = decrypt(&key, &[0; 24], about, &self.sealed)?;

        Some(Secret(secret.try_into().ok()?))
    }
}
