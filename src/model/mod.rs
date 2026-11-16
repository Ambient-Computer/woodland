//! Models: application data, derived from the log's timeline.
//!
//! A model knows nothing about groups, quorums or peers. It declares
//! a few things, and everything else follows from the timeline. About
//! each of its operations:
//!
//! - what it **writes**: values under keys, or the absence of them
//!   ([`Write`]);
//! - what it **requires** of the model's state at its transaction's place
//!   in the timeline: that a key is vacant, that it's present, or that
//!   it holds a certain value ([`Requirement`]). A transaction with a
//!   requirement that doesn't hold is **pruned**: it stays in the log,
//!   and none of its operations happen.
//!
//! And about itself: whether readers may see writes that **haven't
//! committed** ([`ModelId::shows_uncommitted`]).
//!
//! A key holds whatever the last transaction that wasn't pruned wrote
//! there. The kinds of model differ only in what they declare:
//!
//! - `kv` requires nothing, so the last writer wins.
//! - `claims` requires the key to be vacant, so the first writer wins.
//! - A `table` requires what keeps rows and schema coherent: a new
//!   row's table and columns must be present, its key vacant, and the
//!   values it has in unique columns free. A change to a row requires
//!   it to be the row its author read.
//!
//! Any transaction can also require a key to hold what its author read
//! (`Change::Expect`), whatever the model. That's what makes a write
//! conditional, and a claim a lock.
//! - A **threshold** map takes the same operations as `kv`, but only
//!   shows what is committed. A reader never sees a value that some
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
//! # Many models per group
//!
//! A group can hold any number of models, and a transaction can
//! change several of them at once: each operation names the model it's
//! for. The group's clock counts transactions, not models, so one
//! version vector (and one committed cut) covers every model. A model is
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

mod allowance;
mod claims;
pub(crate) mod extension;
mod kv;
pub(crate) mod query;
mod sequence;
pub(crate) mod table;

use serde::{Deserialize, Serialize};

pub use allowance::{Allowance, Allowances};
pub use extension::{Combine, Declared, Extension, Needs, extend};
pub use query::Query;
pub use sequence::Sequence;
pub use table::{ColumnId, Filter, Row, Table, Value};

use crate::actor::Peer;
use crate::db::ClockId;
use crate::encoding;
use crate::store::{Entry, StoreError};

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
    /// Boxed because it names devices by their whole public keys.
    Allowance(Box<allowance::Operation>),
    Table(table::Operation),
    Sequence(sequence::Operation),
    /// A change to a table that's worked out when it's committed, and
    /// does nothing until then.
    Deferred(table::Deferred),
    /// Changes nothing, and requires something of a key, in any kind of
    /// model. It's what makes the rest of its transaction conditional.
    Expect {
        key: Vec<u8>,
        requirement: Requirement,
    },
    /// Drops the model, whatever its kind: everything in it is gone, and
    /// it's dead for good. Every operation requires its model to be
    /// alive, so a transaction that names it later in the timeline is
    /// pruned.
    Drop,
    /// A change to a model of a kind an application added, as only
    /// that kind reads it.
    Custom(Vec<u8>),
}

/// What an operation needs to be true of a key, at its transaction's
/// place in the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Requirement {
    /// The key holds no value. It's how only one transaction gets
    /// something: a name, a row's key, a lock.
    Vacant,
    /// The key holds a value. It's how a transaction depends on another:
    /// if whatever wrote the key was pruned, so is this.
    Present,
    /// The key holds exactly this value. It's how a transaction depends
    /// on what its author read: if something else got there first, in
    /// the timeline, this doesn't happen.
    Is(Fingerprint),
}

/// Stands for a value where only "is it the same one" matters: a hash
/// of it, short enough to keep for every write that isn't committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Fingerprint([u8; 16]);

impl Fingerprint {
    pub(crate) fn of(value: &[u8]) -> Self {
        let hash = encoding::hash("woodland/value", &value);
        let mut fingerprint = [0; 16];
        fingerprint.copy_from_slice(&hash[..16]);

        Self(fingerprint)
    }
}

/// What a model reads of its own committed state, to work out a
/// change that waited for it.
pub(crate) trait Reads: Sized {
    /// The model whose state this is.
    fn model(&self) -> ModelId;

    /// The committed state of another model at the same place in the
    /// timeline, which has to be in the same clock. A model that was
    /// dropped reads as empty.
    fn of(&self, model: ModelId) -> Option<Self>;

    fn get(&self, key: &[u8]) -> impl Future<Output = Result<Option<Vec<u8>>, StoreError>>;

    /// Everything under the keys in `start..end`, in key order.
    fn scan(
        &self,
        start: &[u8],
        end: &[u8],
    ) -> impl Future<Output = Result<Vec<Entry>, StoreError>>;

    /// The start of a [`scan`], from looking at about `limit` keys.
    ///
    /// [`scan`]: Self::scan
    fn page(
        &self,
        start: &[u8],
        end: &[u8],
        limit: usize,
    ) -> impl Future<Output = Result<Page, StoreError>>;
}

/// The start of a scan.
#[derive(Debug, Default)]
pub(crate) struct Page {
    pub(crate) found: Vec<Entry>,
    /// The last key looked at, if there may be more after it. Nothing
    /// found is past it.
    pub(crate) reach: Option<Vec<u8>>,
}

/// What a change that waited to be committed comes to: requirements
/// and writes like any other operation's, in the order they apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    /// The model it's done to: the operation's own, or another in its
    /// clock.
    pub(crate) model: ModelId,
    pub(crate) requires: Vec<(Vec<u8>, Requirement)>,
    pub(crate) writes: Vec<(Vec<u8>, Option<Vec<u8>>)>,
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
        /// What the key holds afterwards, if anything.
        leaves: Option<Fingerprint>,
    },
    /// Drops the model.
    Drop,
}

/// The first version of the library in which a table has a name.
const NAMES: u8 = 7;

/// The latest of everything this version of the library can read.
const LATEST: u8 = NAMES;

/// How much this version of the library can read. Each kind of model
/// has a number, and so does each operation added to a kind since.
/// They're only ever added, so a later version reads everything an
/// earlier one did.
#[cfg(not(test))]
pub(crate) fn vocabulary() -> u8 {
    LATEST
}

#[cfg(test)]
thread_local! {
    static VOCABULARY: std::cell::Cell<u8> = const { std::cell::Cell::new(LATEST) };
}

#[cfg(test)]
pub(crate) fn vocabulary() -> u8 {
    VOCABULARY.get()
}

/// Makes this thread run as a version of the library from before
/// allowances were added, or as the latest.
#[cfg(test)]
pub(crate) fn predate(old: bool) {
    let latest = if old { Kind::Table as u8 } else { LATEST };

    VOCABULARY.set(latest);
}

/// What a key of a merging model holds once `write` is combined with
/// what it held.
pub(crate) fn combine(model: ModelId, held: Option<&[u8]>, write: &[u8]) -> Vec<u8> {
    match model.extension().and_then(|kind| kind.combine) {
        Some(combine) => combine(held, write),
        None => sequence::combine(held, write),
    }
}

/// Set in the first byte of the ID of a model that's restricted to
/// committed reads.
const HELD: u8 = 0x80;

/// The kinds of model there are. A model's kind is part of its ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Kv = 1,
    Claims = 2,
    Threshold = 3,
    Table = 4,
    Allowances = 5,
    Sequence = 6,
}

impl Kind {
    /// Whether readers of this kind see writes that aren't committed,
    /// unless the model says otherwise.
    fn shows_uncommitted(self) -> bool {
        !matches!(self, Self::Threshold)
    }

    /// Whether a key holds every write to it combined, rather than
    /// the last.
    pub(crate) fn merges(self) -> bool {
        matches!(self, Self::Sequence)
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

    pub(crate) fn insert_row(model: ModelId, key: Vec<u8>, cells: Vec<(ColumnId, Value)>) -> Self {
        let incarnation = table::UNSEALED;
        let row = table::Target { key, incarnation };
        let change = Change::Table(table::Operation::Insert { row, cells });

        Self { model, change }
    }

    pub(crate) fn table(model: ModelId, operation: table::Operation) -> Self {
        let change = Change::Table(operation);

        Self { model, change }
    }

    /// Fills in what only the transaction's author can work out, now
    /// that it's known which of its transactions this is part of.
    pub(crate) fn seal(&mut self, author: Peer, seq: u64, index: usize) {
        match &mut self.change {
            Change::Table(operation) => operation.seal(author, seq, index),
            Change::Allowance(operation) => operation.seal(author, seq, index),
            Change::Sequence(operation) => operation.seal(author, seq, index),
            _ => {}
        }
    }

    pub(crate) fn sequence(model: ModelId, key: Vec<u8>, edit: sequence::Edit) -> Self {
        let change = Change::Sequence(sequence::Operation::new(key, edit));

        Self { model, change }
    }

    /// Whether what the operation writes is combined with what its
    /// key holds, rather than taking its place.
    pub(crate) fn merges(&self) -> bool {
        match self.change {
            Change::Sequence(_) => true,
            Change::Custom(_) => self.model.merges(),
            _ => false,
        }
    }

    pub(crate) fn allowance(model: ModelId, operation: allowance::Operation) -> Self {
        let change = Change::Allowance(Box::new(operation));

        Self { model, change }
    }

    /// Whether `author` may make the change. Most changes are anyone's
    /// who can write to the clock.
    fn permits(&self, author: Peer) -> bool {
        match &self.change {
            Change::Allowance(operation) => operation.permits(author),
            _ => true,
        }
    }

    pub(crate) fn name_column(model: ModelId, column: ColumnId, name: String, add: bool) -> Self {
        let change = Change::Table(table::Operation::NameColumn { column, name, add });

        Self { model, change }
    }

    pub(crate) fn drop_column(model: ModelId, column: ColumnId) -> Self {
        let change = Change::Deferred(table::Deferred::DropColumn { column });

        Self { model, change }
    }

    pub(crate) fn deferred(model: ModelId, deferred: table::Deferred) -> Self {
        let change = Change::Deferred(deferred);

        Self { model, change }
    }

    /// The change to work out when the operation is committed, if
    /// it's one that waits.
    pub(crate) fn waits(&self) -> Option<&table::Deferred> {
        match &self.change {
            Change::Deferred(deferred) => Some(deferred),
            _ => None,
        }
    }

    pub(crate) fn expect(model: ModelId, key: Vec<u8>, requirement: Requirement) -> Self {
        let change = Change::Expect { key, requirement };

        Self { model, change }
    }

    pub(crate) fn release(model: ModelId, key: Vec<u8>, value: Vec<u8>) -> Self {
        let change = Change::Claims(claims::Operation::Release { key, value });

        Self { model, change }
    }

    pub(crate) fn custom(model: ModelId, operation: Vec<u8>) -> Self {
        let change = Change::Custom(operation);

        Self { model, change }
    }

    /// What an application's kind says the operation does, if it's
    /// for a model of one this device has, and is one of its own.
    fn declared(&self) -> Option<Declared> {
        let Change::Custom(operation) = &self.change else {
            return None;
        };

        (self.model.extension()?.declare)(operation)
    }

    pub(crate) fn drop(model: ModelId) -> Self {
        let change = Change::Drop;

        Self { model, change }
    }

    /// The version of the library that first had this operation, if
    /// it's later than the one that first had its kind. Zero if not.
    pub(crate) fn since(&self) -> u8 {
        match &self.change {
            Change::Table(table::Operation::Name { .. }) => NAMES,
            _ => 0,
        }
    }

    /// Whether the change is one its model takes.
    pub(crate) fn belongs(&self) -> bool {
        match (&self.change, self.model.kind()) {
            (Change::Kv(_), Some(Kind::Kv | Kind::Threshold))
            | (Change::Claims(_), Some(Kind::Claims))
            | (Change::Sequence(_), Some(Kind::Sequence))
            | (Change::Expect { .. } | Change::Drop, Some(_)) => true,
            (Change::Allowance(operation), Some(Kind::Allowances)) => operation.belongs(),
            (Change::Table(operation), Some(Kind::Table)) => operation.belongs(),
            (Change::Deferred(deferred), Some(Kind::Table)) => deferred.belongs(),
            (Change::Expect { .. } | Change::Drop, None) => self.model.extension().is_some(),
            (Change::Custom(_), None) => self.declared().is_some(),
            (
                Change::Kv(_)
                | Change::Claims(_)
                | Change::Allowance(_)
                | Change::Table(_)
                | Change::Sequence(_)
                | Change::Deferred(_)
                | Change::Custom(_),
                _,
            ) => false,
        }
    }

    /// Everything the operation declares, without its values: what it
    /// requires, then what it writes.
    pub(crate) fn declares(&self, author: Peer) -> Vec<Declaration> {
        let requires = self.requires().into_iter();
        let requires = requires.map(|(key, requirement)| Effect::Require { key, requirement });
        let writes = self.writes().into_iter().map(|write| Effect::Write {
            key: write.key,
            leaves: write.value.as_deref().map(Fingerprint::of),
        });

        let drops = self.drops().then_some(Effect::Drop);
        let belongs = self.belongs() && self.permits(author);
        // One that isn't its model's to take may say nothing at all,
        // and still takes its transaction with it.
        let stray = || Effect::Require {
            key: Vec::new(),
            requirement: Requirement::Present,
        };
        let effects: Vec<Effect> = requires.chain(writes).chain(drops).collect();
        let stray = (!belongs && effects.is_empty()).then(stray);
        let effects = effects.into_iter().chain(stray);
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
            Change::Allowance(operation) => operation.writes(),
            Change::Table(operation) => operation.writes(),
            Change::Sequence(operation) => vec![operation.write()],
            Change::Expect { .. } | Change::Drop | Change::Deferred(_) => Vec::new(),
            Change::Custom(_) => self.declared().unwrap_or_default().writes,
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
            Change::Kv(_) | Change::Sequence(_) | Change::Drop | Change::Deferred(_) => Vec::new(),
            Change::Claims(operation) => vec![operation.requires()],
            Change::Expect { key, requirement } => vec![(key.clone(), *requirement)],
            Change::Allowance(operation) => operation.requires(),
            Change::Table(operation) => operation.requires(),
            Change::Custom(_) => {
                let requires = self.declared().unwrap_or_default().requires;
                let requirement = |(key, needs): (Vec<u8>, Needs)| (key, needs.into());

                requires.into_iter().map(requirement).collect()
            }
        }
    }
}

/// What one operation leaves under a key, at its transaction's place in
/// the timeline. Both the key and the value are the model's to interpret.
///
/// It's kept as a version of the key until the transaction is committed, and
/// then becomes the key's committed value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Write {
    pub(crate) model: ModelId,
    pub(crate) key: Vec<u8>,
    /// `None` leaves the key without a value.
    pub(crate) value: Option<Vec<u8>>,
}

/// Names a model: the clock that owns it, then its kind and the BLAKE3
/// hash of the clock's ID and a nonce.
///
/// Like a group ID, it's derived from how the model began, so models
/// created independently on different devices never collide. The model's
/// initial state is always empty, so the nonce is the only input that varies.
///
/// A group's first clock is named after the group, so a model can be
/// made from a [`GroupId`] alone.
///
/// [`GroupId`]: crate::GroupId
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ModelId {
    clock: ClockId,
    id: [u8; 32],
}

impl ModelId {
    /// A key-value map: the last write to a key wins. The application
    /// supplies the nonce: random bytes.
    #[must_use]
    pub fn kv(clock: impl Into<ClockId>, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Kv, clock.into(), nonce)
    }

    /// A map of claims: the first claim on a key wins, and a transaction
    /// that loses one doesn't happen at all.
    #[must_use]
    pub fn claims(clock: impl Into<ClockId>, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Claims, clock.into(), nonce)
    }

    /// A key-value map that only shows what is committed: values every
    /// contributor holds, in an order that's final. Reads of it never go
    /// back. Written like a [key-value] map.
    ///
    /// [key-value]: ModelId::kv
    #[must_use]
    pub fn threshold(clock: impl Into<ClockId>, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Threshold, clock.into(), nonce)
    }

    /// Allowances: quantities that several devices spend from, each
    /// out of its own share, so that none is ever overspent. See
    /// [`Transaction::open_allowance`].
    ///
    /// [`Transaction::open_allowance`]: crate::Transaction::open_allowance
    #[must_use]
    pub fn allowances(clock: impl Into<ClockId>, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Allowances, clock.into(), nonce)
    }

    /// Sequences: ordered lists that merge. What two devices put into
    /// one list while apart is all there when they meet, where each
    /// put it. See [`Transaction::insert_at`].
    ///
    /// [`Transaction::insert_at`]: crate::Transaction::insert_at
    #[must_use]
    pub fn sequence(clock: impl Into<ClockId>, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Sequence, clock.into(), nonce)
    }

    /// A model of a kind the application added with [`extend`], by the
    /// number it was added under. `None` if the number is 64 or more.
    #[must_use]
    pub fn of_kind(number: u8, clock: impl Into<ClockId>, nonce: [u8; 32]) -> Option<Self> {
        let clock = clock.into();
        let mut id = encoding::hash("woodland/model", &(clock.as_bytes(), nonce));
        id[0] = extension::FIRST
            .checked_add(number)
            .filter(|it| it & HELD == 0)?;

        Some(Self { clock, id })
    }

    /// A table: rows of typed cells, in columns that are themselves
    /// part of the data. It exists once a transaction [creates] it.
    ///
    /// [creates]: crate::Transaction::create_table
    #[must_use]
    pub fn table(clock: impl Into<ClockId>, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Table, clock.into(), nonce)
    }

    /// The same kind of model, restricted to committed reads: nobody
    /// ever reads a value from it that could still be displaced, its
    /// own author included. It's a different model from the one this
    /// started as, with its own keys.
    #[must_use]
    pub fn committed_only(mut self) -> Self {
        self.id[0] |= HELD;
        self
    }

    /// Whether readers see writes that aren't committed. If not, they
    /// aren't kept as versions at all: they wait in the log.
    pub(crate) fn shows_uncommitted(&self) -> bool {
        let shows = self
            .kind()
            .map_or(self.extension().is_some(), Kind::shows_uncommitted);

        self.id[0] & HELD == 0 && shows
    }

    /// `None` for an ID this version of the library didn't make.
    pub(crate) fn kind(&self) -> Option<Kind> {
        [
            Kind::Kv,
            Kind::Claims,
            Kind::Threshold,
            Kind::Table,
            Kind::Allowances,
            Kind::Sequence,
        ]
        .into_iter()
        .find(|&kind| kind as u8 == self.id[0] & !HELD && kind as u8 <= vocabulary())
    }

    /// The kind an application added that the model is of, if this
    /// device has it.
    pub(crate) fn extension(&self) -> Option<Extension> {
        extension::of(self.id[0] & !HELD)
    }

    /// Whether a key of the model holds every write to it combined,
    /// rather than the last.
    pub(crate) fn merges(&self) -> bool {
        let extended = self.extension().is_some_and(|kind| kind.combine.is_some());

        self.kind().is_some_and(Kind::merges) || extended
    }

    /// Whether the model is of a kind this device has.
    pub(crate) fn is_known(&self) -> bool {
        self.kind().is_some() || self.extension().is_some()
    }

    /// The clock the model belongs to. A transaction writes to the
    /// models of one clock.
    #[must_use]
    pub fn clock(&self) -> ClockId {
        self.clock
    }

    fn new(kind: Kind, clock: ClockId, nonce: [u8; 32]) -> Self {
        let mut id = encoding::hash("woodland/model", &(clock.as_bytes(), nonce));
        id[0] = kind as u8;

        Self { clock, id }
    }

    /// The clock, then the model within it: a clock's models are one
    /// range of keys.
    pub(crate) fn as_bytes(&self) -> [u8; 64] {
        let mut bytes = [0; 64];
        bytes[..32].copy_from_slice(self.clock.as_bytes());
        bytes[32..].copy_from_slice(&self.id);

        bytes
    }

    pub(crate) fn from_bytes(bytes: [u8; 64]) -> Self {
        let mut clock = [0; 32];
        let mut id = [0; 32];
        clock.copy_from_slice(&bytes[..32]);
        id.copy_from_slice(&bytes[32..]);

        Self {
            clock: ClockId::from_bytes(clock),
            id,
        }
    }
}

/// Abbreviated, like a git hash.
impl std::fmt::Debug for ModelId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in &self.id[..4] {
            write!(formatter, "{byte:02x}")?;
        }

        Ok(())
    }
}
