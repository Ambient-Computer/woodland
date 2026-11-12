//! Models: application data, derived from the log's timeline.
//!
//! A model knows nothing about agreements, quorums or peers. It says what
//! the operations of a verified, authorized transaction leave behind at
//! that transaction's place in the timeline. What it relies on from the
//! layers above:
//!
//! - **Every transaction has a unique [`Stamp`].** The author's Lamport
//!   counter plus the author's key. Sorting by stamp is the timeline: one
//!   order every device agrees on, in which a transaction always comes
//!   after everything it was written on top of.
//! - **Transactions are atomic.** All of a transaction's operations share
//!   its stamp and are written together, or not at all.
//!
//! # Many models per agreement
//!
//! An agreement can hold any number of models. The agreement's clock counts
//! transactions across all of them, so one version vector (and one
//! compaction floor) covers every model at once. A model is just a
//! namespace inside that history: creating one is choosing an ID, and the
//! first transaction that names it brings it into existence everywhere.
//!
//! [`Stamp`]: crate::clock::Stamp

mod kv;
mod query;

use serde::{Deserialize, Serialize};

pub(crate) use kv::{Operation, Version};
pub use query::Query;

use crate::agreement::AgreementId;
use crate::encoding;

/// Names a model within an agreement. The BLAKE3 hash of the agreement's ID
/// and a nonce.
///
/// Like an agreement ID, it's derived from how the model began, so models
/// created independently on different devices never collide. The model's
/// initial state is always empty, so the nonce is the only input that varies.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ModelId([u8; 32]);

impl ModelId {
    /// The application supplies the nonce, as with [`Genesis`].
    ///
    /// [`Genesis`]: crate::Genesis
    #[must_use]
    pub fn new(agreement: AgreementId, nonce: [u8; 32]) -> Self {
        Self(encoding::hash(
            "woodland/model/kv",
            &(agreement.as_bytes(), nonce),
        ))
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

/// Abbreviated, like a git hash.
impl std::fmt::Debug for ModelId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in &self.0[..4] {
            write!(formatter, "{byte:02x}")?;
        }

        Ok(())
    }
}
