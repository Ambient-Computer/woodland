//! Models: application data, derived from the log's timeline.
//!
//! A model knows nothing about agreements, quorums or peers. It declares
//! a few things, and everything else follows from the timeline. About
//! each of its operations:
//!
//! - what it **writes**: values under keys, or the absence of them
//!   ([`Write`]);
//! - what it **requires** of the model's state at its transaction's place
//!   in the timeline: that a key is vacant, or that it's present
//!   ([`Requirement`]). A transaction with a requirement that doesn't
//!   hold is **pruned**: it stays in the log, and none of its operations
//!   happen.
//!
//! And about itself: whether readers may see writes that **haven't
//! settled** ([`Kind::shows_unsettled`]).
//!
//! A key holds whatever the last transaction that wasn't pruned wrote
//! there. The kinds of model differ only in what they declare:
//!
//! - `kv` requires nothing, so the last writer wins.
//! - `claims` requires the key to be vacant, so the first writer wins.
//! - A `table` requires what keeps rows and schema coherent: a row's
//!   table and columns must be present, and its key vacant.
//! - A **threshold** map takes the same operations as `kv`, but only
//!   shows what has settled. A reader never sees a value that some
//!   contributor might not have, or one that an earlier write could
//!   still displace, and what it sees never goes back. It's for values
//!   that others act on: a schema version to migrate to, a key to
//!   rotate to, a checkpoint.
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
//! Dropping a model is an operation too, at a place in the timeline like
//! any other. Before it, the model's operations happen, and the drop
//! then takes what they wrote. After it, the model is dead for good, and
//! a transaction that names it is pruned.
//!
//! [`Stamp`]: crate::clock::Stamp

mod claims;
mod kv;
mod query;
mod table;

use serde::{Deserialize, Serialize};

pub use query::Query;
pub use table::{ColumnId, Row, Table, Value};

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
    Table(table::Operation),
    /// Drops the model, whatever its kind: everything in it is gone, and
    /// it's dead for good. Every operation requires its model to be
    /// alive, so a transaction that names it later in the timeline is
    /// pruned.
    Drop,
}

/// What an operation needs to be true of a key, at its transaction's
/// place in the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Requirement {
    /// The key holds no value. It's how only one transaction gets
    /// something: a name, a row's key, a lock.
    Vacant,
    /// The key holds a value. It's how a transaction depends on another:
    /// if whatever wrote the key was pruned, so is this.
    Present,
}

/// One thing an operation declares, apart from the values it carries.
/// An operation declares a list of them, in the order they apply, and
/// that's everything the layers above need to know of it, so it's all
/// they keep once the transaction is in storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Declaration {
    pub(crate) model: ModelId,
    /// Whether the change is one its model takes.
    pub(crate) belongs: bool,
    pub(crate) effect: Effect,
}

/// What an operation does to its model, or needs of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Needs something of a key. If it doesn't hold, the transaction is
    /// pruned.
    Require {
        key: Vec<u8>,
        requirement: Requirement,
    },
    /// Writes to a key.
    Write {
        key: Vec<u8>,
        /// Whether the key holds a value afterwards.
        leaves_value: bool,
    },
    /// Drops the model.
    Drop,
}

/// The kinds of model there are. A model's kind is part of its ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Kv = 1,
    Claims = 2,
    Threshold = 3,
    Table = 4,
}

impl Kind {
    /// Whether readers see writes that haven't settled. If not, they
    /// aren't kept as versions at all: they wait in the log.
    pub(crate) fn shows_unsettled(self) -> bool {
        !matches!(self, Self::Threshold)
    }
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

    pub(crate) fn create_table(model: ModelId, columns: Vec<(ColumnId, String)>) -> Self {
        let change = Change::Table(table::Operation::Create { columns });

        Self { model, change }
    }

    pub(crate) fn insert(model: ModelId, key: Vec<u8>, cells: Vec<(ColumnId, Value)>) -> Self {
        let change = Change::Table(table::Operation::Insert { key, cells });

        Self { model, change }
    }

    pub(crate) fn drop(model: ModelId) -> Self {
        let change = Change::Drop;

        Self { model, change }
    }

    /// Whether the change is one its model takes.
    pub(crate) fn belongs(&self) -> bool {
        match (&self.change, self.model.kind()) {
            (Change::Kv(_), Some(Kind::Kv | Kind::Threshold))
            | (Change::Claims(_), Some(Kind::Claims))
            | (Change::Drop, Some(_)) => true,
            (Change::Table(operation), Some(Kind::Table)) => operation.belongs(),
            (Change::Kv(_) | Change::Claims(_) | Change::Table(_) | Change::Drop, _) => false,
        }
    }

    /// Everything the operation declares, without its values: what it
    /// requires, then what it writes.
    pub(crate) fn declares(&self) -> Vec<Declaration> {
        let requires = self.requires().into_iter();
        let requires = requires.map(|(key, requirement)| Effect::Require { key, requirement });
        let writes = self.writes().into_iter().map(|write| Effect::Write {
            key: write.key,
            leaves_value: write.value.is_some(),
        });

        let drops = self.drops().then_some(Effect::Drop);
        let effects = requires.chain(writes).chain(drops);
        let belongs = self.belongs();
        let declare = |effect| Declaration {
            model: self.model,
            belongs,
            effect,
        };

        effects.map(declare).collect()
    }

    /// What the operation leaves under each key it writes to, in order.
    pub(crate) fn writes(&self) -> Vec<Write> {
        let writes = match &self.change {
            Change::Kv(operation) => vec![operation.write()],
            Change::Claims(operation) => vec![operation.write()],
            Change::Table(operation) => operation.writes(),
            Change::Drop => Vec::new(),
        };

        let write = |(key, value)| Write {
            model: self.model,
            key,
            value,
        };

        writes.into_iter().map(write).collect()
    }

    /// Whether the operation drops its model.
    pub(crate) fn drops(&self) -> bool {
        matches!(self.change, Change::Drop)
    }

    /// What must hold, and of which keys, for the operation's
    /// transaction to happen.
    pub(crate) fn requires(&self) -> Vec<(Vec<u8>, Requirement)> {
        match &self.change {
            Change::Kv(_) | Change::Drop => Vec::new(),
            Change::Claims(operation) => vec![operation.requires()],
            Change::Table(operation) => operation.requires(),
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

    /// A key-value map that only shows what has settled: values every
    /// contributor holds, in an order that's final. Reads of it never go
    /// back. Written like a [key-value] map.
    ///
    /// [key-value]: ModelId::kv
    #[must_use]
    pub fn threshold(agreement: AgreementId, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Threshold, agreement, nonce)
    }

    /// A table: rows of typed cells, in columns that are themselves
    /// part of the data. It exists once a transaction [creates] it.
    ///
    /// [creates]: crate::TransactionBuilder::create_table
    #[must_use]
    pub fn table(agreement: AgreementId, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Table, agreement, nonce)
    }

    /// `None` for an ID this version of the library didn't make.
    pub(crate) fn kind(&self) -> Option<Kind> {
        [Kind::Kv, Kind::Claims, Kind::Threshold, Kind::Table]
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
