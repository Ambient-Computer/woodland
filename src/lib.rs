//! Woodland: a peer-to-peer relational database for local-first apps.
//!
//! A few devices share a [group]. Each writes whenever it likes, online
//! or not, and reads what it has. A quorum of them commits history in
//! checkpoints, and what's committed is final: the same on every device,
//! never reordered, never undone.
//!
//! `docs/spec` is the design, and `docs/context` is why. Read those
//! first. This is where each part of the design lives:
//!
//! 1. [`Database`] (`db/`): every group one device holds, over one
//!    [`Store`]. The application hands it what happens (a write, a
//!    message, the passing of time) and gets back what to send and what
//!    changed. Each call is one step, and one atomic batch.
//! 2. `db/group/`: one group. Its chain, its clocks, and what each
//!    peer lacks. A [clock] (`db/group/clock.rs`) is one history:
//!    everyone's entries, which of them are committed, and which of
//!    the rest happen.
//! 3. `chain/`: the checkpoint chain. One decision at a time, each made
//!    by the electorate the one before it named.
//! 4. `model/`: what operations write and what they require. A
//!    key-value map, a map of claims, a map that only shows what's
//!    committed, and tables (`model/table/`).
//! 5. `store/`: the storage contract, and the stores that meet it.
//! 6. `actor/`, `clock/`, `encoding`: identities and signatures, the
//!    stamps that order entries, and how things become bytes.
//!
//! [group]: GroupId
//! [clock]: ClockId

mod actor;
mod blob;
mod chain;
mod clock;
mod db;
mod encoding;
mod model;
mod seal;
mod store;
#[cfg(test)]
mod testing;

pub use actor::Identity;
pub use blob::BlobRef;
pub use chain::Role;
pub use db::{
    Answer, Changed, ClockId, Configuration, Database, Error, Found, GroupId, Outcome, Outputs,
    Part, Pending, Reached, Reconfigured, Transaction, TransactionId, Waiting,
};
#[cfg(feature = "blobs")]
pub use db::{BlobPolicy, Import, ImportId};
pub use model::{
    Allowance, Allowances, ColumnId, Combine, Comparison, Declared, Extension, Filter, ModelId,
    Needs, Query, Row, Sequence, Table, Value, extend,
};
pub use seal::SealingKey;
#[cfg(all(feature = "browser", target_arch = "wasm32"))]
pub use store::{BrowserStore, BrowserView};
pub use store::{
    Entry, MemoryStore, MemoryView, Order, Store, StoreError, StoreFault, View, Write,
};
#[cfg(feature = "file")]
pub use store::{FileStore, FileView};
