//! Woodland: a replicated database for devices that are mostly offline.
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
//! 2. `db/group/`: one group. Everyone's entries, which of them are
//!    committed, which of the rest happen, and what each peer lacks.
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

mod actor;
mod chain;
mod clock;
mod db;
mod encoding;
mod model;
mod store;
#[cfg(test)]
mod testing;

pub use actor::Identity;
pub use db::{Changed, Database, Error, GroupId, Outputs, Transaction};
pub use model::{ColumnId, ModelId, Query, Row, Table, Value};
pub use store::{Entry, MemoryStore, MemoryView, Order, Store, StoreError, View, Write};
