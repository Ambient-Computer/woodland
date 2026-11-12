//! Models: application data, derived from the log's timeline.
//!
//! A model knows nothing about agreements, quorums or peers. It declares
//! two things about each of its operations, and everything else follows
//! from the timeline:
//!
//! - what it **writes**: a value under a key, or the absence of one
//!   ([`Write`]);
//! - what it **requires** of the model's state at its transaction's place
//!   in the timeline ([`Requirement`]). A transaction with a requirement
//!   that doesn't hold is **pruned**: it stays in the log, and none of
//!   its operations happen.
//!
//! A key holds whatever the last transaction that wasn't pruned wrote
//! there. The models differ in their rules:
//!
//! - `kv` requires nothing, so the last writer wins.
//! - `claims` requires the key to be vacant, so the first writer wins.
//!
//! What a model relies on from the layers above:
//!
//! - **Every transaction has a unique [`Stamp`].** The author's Lamport
//!   counter plus the author's key. Sorting by stamp is the timeline: one
//!   order every device agrees on, in which a transaction always comes
//!   after everything it was written on top of.
//! - **Transactions are atomic.** All of a transaction's operations share
//!   its stamp and are written together, or not at all, whichever models
//!   they're for.
//!
//! # What a model owns
//!
//! A model owns its operations and the meaning of what it stores. To the
//! log, the group and storage, an operation is something to carry, and
//! what it declares is all they know of it: bytes under a key, in the
//! model's own range, at the transaction's stamp, and whether the key
//! had to be vacant. Only the model and its [`Query`] read them back.
//!
//! A model's ID says which kind it is, and an operation of another kind
//! doesn't belong to it. A transaction holding one is pruned, so a `Set`
//! can never take a claimed key.
//!
//! # Many models per agreement
//!
//! An agreement can hold any number of models, and a transaction can
//! change several of them at once: each operation names the model it's
//! for. The agreement's clock counts transactions, not models, so one
//! version vector (and one settled cut) covers every model. A model is
//! just a namespace inside that history: creating one is choosing an ID,
//! and the first operation that names it brings it into existence
//! everywhere. Two models never see each other's keys.
//!
//! [`Stamp`]: crate::clock::Stamp

mod claims;
mod kv;
mod query;

use serde::{Deserialize, Serialize};

pub use query::Query;

use crate::agreement::AgreementId;
use crate::encoding;

/// One step of a transaction: a change to one model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Operation {
    pub(crate) model: ModelId,
    pub(crate) change: Change,
}

/// A change, in the vocabulary of the kind of model it's for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Change {
    Kv(kv::Operation),
    Claims(claims::Operation),
}

/// What an operation needs to be true of its key, at its transaction's
/// place in the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Requirement {
    /// The key holds no value.
    Vacant,
}

/// The kinds of model there are. A model's kind is part of its ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Kv = 1,
    Claims = 2,
}

impl Operation {
    pub(crate) fn set(model: ModelId, key: Vec<u8>, value: Vec<u8>) -> Self {
        let change = Change::Kv(kv::Operation::Set { key, value });

        Self { model, change }
    }

    pub(crate) fn remove(model: ModelId, key: Vec<u8>) -> Self {
        let change = Change::Kv(kv::Operation::Remove { key });

        Self { model, change }
    }

    pub(crate) fn claim(model: ModelId, key: Vec<u8>, value: Vec<u8>) -> Self {
        let change = Change::Claims(claims::Operation::Claim { key, value });

        Self { model, change }
    }

    /// Whether the change is one its model takes.
    pub(crate) fn belongs(&self) -> bool {
        let kind = match self.change {
            Change::Kv(_) => Kind::Kv,
            Change::Claims(_) => Kind::Claims,
        };

        self.model.kind() == Some(kind)
    }

    pub(crate) fn write(&self) -> Write {
        let (key, value) = match &self.change {
            Change::Kv(operation) => operation.write(),
            Change::Claims(operation) => operation.write(),
        };

        Write {
            model: self.model,
            key,
            value,
        }
    }

    /// What must hold of the operation's key for its transaction to
    /// happen.
    pub(crate) fn requires(&self) -> Option<Requirement> {
        match &self.change {
            Change::Kv(_) => None,
            Change::Claims(operation) => Some(operation.requires()),
        }
    }
}

/// What one operation leaves under a key, at its transaction's place in
/// the timeline. Both the key and the value are the model's to interpret.
///
/// It's kept as a version of the key until the transaction settles, and
/// then becomes the key's settled value (see `group::settling`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Write {
    pub(crate) model: ModelId,
    pub(crate) key: Vec<u8>,
    /// `None` leaves the key without a value.
    pub(crate) value: Option<Vec<u8>>,
}

/// Names a model within an agreement: its kind, then the BLAKE3 hash of
/// the agreement's ID and a nonce.
///
/// Like an agreement ID, it's derived from how the model began, so models
/// created independently on different devices never collide. The model's
/// initial state is always empty, so the nonce is the only input that varies.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ModelId([u8; 32]);

impl ModelId {
    /// A key-value map: the last write to a key wins. The application
    /// supplies the nonce, as with [`Genesis`].
    ///
    /// [`Genesis`]: crate::Genesis
    #[must_use]
    pub fn kv(agreement: AgreementId, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Kv, agreement, nonce)
    }

    /// A map of claims: the first claim on a key wins, and a transaction
    /// that loses one doesn't happen at all.
    #[must_use]
    pub fn claims(agreement: AgreementId, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Claims, agreement, nonce)
    }

    /// `None` for an ID this version of the library didn't make.
    pub(crate) fn kind(&self) -> Option<Kind> {
        [Kind::Kv, Kind::Claims]
            .into_iter()
            .find(|&kind| kind as u8 == self.0[0])
    }

    fn new(kind: Kind, agreement: AgreementId, nonce: [u8; 32]) -> Self {
        let mut id = encoding::hash("woodland/model", &(agreement.as_bytes(), nonce));
        id[0] = kind as u8;

        Self(id)
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
