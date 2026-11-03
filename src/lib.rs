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
//!
//! # What this experiment found
//!
//! **Protocol interactions**
//!
//! - RLA works as pure state-based gossip. Replicas witness by holding an
//!   identical view instead of answering numbered rounds, which suits
//!   devices that are offline for weeks (`agreement/`).
//! - A new configuration can't vouch for itself. A quorum of every
//!   configuration in play must agree. The safety proptest finds a
//!   counterexample within seconds without this
//!   (`a_reconfiguration_needs_the_old_quorum_too`).
//! - "Persist before send" is a safety requirement of RLA, not just a
//!   data-loss concern. A replica that forgets a view it sent can witness
//!   two incomparable values (`a_replica_that_forgets_breaks_agreement`).
//! - Continuous proposals starve learning, in practice as well as in
//!   theory. One progress report in flight per contributor fixes it
//!   (`Group::may_report`).
//! - Witnesses don't need to be heard directly. A signed "I hold digest D"
//!   stays true forever, because views are durable and only grow, so it can
//!   be relayed like a transaction (`agreement/attestation.rs`). Direct
//!   witnessing was rarely the bottleneck, though: the learner needn't be a
//!   contributor, so any hub that hears a quorum learns for everyone
//!   (`a_hub_that_isnt_a_contributor_learns_for_everyone`).
//! - Writing never needs a quorum. Only membership changes and compaction
//!   do, so a device offline for months can keep writing.
//!
//! **What composes**
//!
//! - Lattices everywhere: the agreement's value, its configuration, version
//!   vectors and the KV model all merge by join, so one "merge whatever you
//!   hear" rule covers sync, gossip and snapshot catch-up.
//! - The agreement carries clock progress as its value, so the compaction
//!   floor is a GLB over an agreed membership, and models stay ignorant of
//!   both (`group/compaction.rs`).
//! - Many models per agreement: one clock spans them all, and a model is
//!   just a namespace in that history.
//! - Signed transactions and attestations can be relayed by anyone, so sync
//!   and agreement both work over chains of devices, not just a full mesh.
//!   Both go through the same batch verification before a group sees them.
//! - Reads bypass the state machine. Every integrated change is written in
//!   the batch that integrates it, and writes precede every other action,
//!   so storage is current once a batch's actions are performed. A
//!   [`Query`] is just a key range and a decoder.
//! - Compaction bounds hydration. Only protocol state loads at startup, and
//!   the log in it holds only what isn't compacted yet (`storage/keys.rs`).
//!
//! **What fights**
//!
//! - Last-writer-wins and sans-IO. LWW can't apply a write blind: it needs
//!   the current entry's stamp, so integrating is read-modify-write, and
//!   every read is a round trip through the application. Transactions and
//!   snapshots wait across batches for their keys to load (`group/models.rs`,
//!   `an_older_write_still_loses_after_a_restart`). Sending a snapshot is a
//!   storage scan too. A model whose writes commute blindly in storage
//!   (multi-version keys, resolved at read time) would avoid all of this,
//!   and move the cost to reads.
//!
//! - Removal and revocation. RLA gives comparable decisions, not a "first
//!   decision that included this change". There's no deterministic point at
//!   which a removed device's writes stop counting, so removal can't revoke
//!   past or backdated writes (`History::integrate_ready`).
//! - Tombstone collection. The floor is enough to drop the log, but not
//!   enough to drop tombstones, for the same reason plus concurrent
//!   admission of new members (`group/compaction.rs`).
//! - Crash faults only. A lying member can forge progress or equivocate,
//!   and Byzantine RLA is a separate line of research.

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
pub use model::{ModelId, Query};
pub use node::{Batch, Node, TransactionBuilder};
pub use storage::{StorageAction, StorageWrite};
