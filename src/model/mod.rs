//! Models: CRDTs holding application data, changed by signed transactions.
//!
//! A model knows nothing about agreements, quorums or peers. It receives
//! transactions that were already verified and authorized, applies them,
//! and merges snapshots. What it relies on from the layers above:
//!
//! - **Every transaction has a unique [`Stamp`].** The author's Lamport
//!   counter plus the author's key, so last-writer-wins has no ties.
//! - **Transactions are atomic.** All of a transaction's operations share
//!   its stamp and are applied together, or not at all.
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
mod transaction;

use serde::{Deserialize, Serialize};

pub(crate) use kv::{Entry, KvModel, Operation};
pub(crate) use transaction::Transaction;

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
