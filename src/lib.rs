//! Woodland: replicated data for devices that are mostly offline.
//!
//! # Reading order
//!
//! The file structure is the review hierarchy. Each module's entry point
//! describes the flow, and the details live underneath it.
//!
//! 1. [`Node`] (`node/`): the public state machine. Events in, ordered
//!    actions out.
//! 2. `group/`: one agreement and everything it owns. The only place the
//!    other pieces meet.
//! 3. `agreement/`: Reconfigurable Lattice Agreement. Who the contributors
//!    are, and how far each has gotten.
//! 4. `model/`: the KV CRDT, and the signed transactions that change it.
//! 5. `clock/`: version vectors, Lamport stamps, and the greatest lower
//!    bound that drives compaction.
//! 6. `storage/`: the key-value contract and the key layout.
//! 7. `actor/`: identities, peers and signatures.

mod actor;
mod agreement;
mod clock;
mod encoding;
mod group;
mod io;
mod lattice;
mod model;
mod node;
mod storage;
#[cfg(test)]
mod testing;

pub use actor::Identity;
pub use agreement::{AgreementId, Genesis};
pub use io::{Action, NetworkAction};
pub use model::ModelId;
pub use node::{Batch, Node, TransactionBuilder};
pub use storage::{StorageAction, StorageWrite};
