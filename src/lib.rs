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
//! 4. `log/`: signed transactions, delivered in causal order, and the
//!    timeline they sort into.
//! 5. `model/`: what operations write and require. A key-value map,
//!    a map of claims, and a map that only shows what has settled.
//! 6. `clock/`: version vectors, the stamps that order the timeline, and
//!    the greatest lower bound that settling starts from.
//! 7. `storage/`: the key-value contract and the key layout.
//! 8. `actor/`: identities, peers and signatures.
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
//! - Writing never needs a quorum. Only membership changes and settling
//!   do, so a device offline for months can keep writing.
//!
//! **What composes**
//!
//! - Lattices everywhere: the agreement's value, its configuration, version
//!   vectors and the KV model's versions all merge by join, so one "merge
//!   whatever you hear" rule covers sync, gossip and snapshot catch-up.
//! - The agreement carries clock progress as its value, so the floor is a
//!   GLB over an agreed membership, and models stay ignorant of both.
//! - The floor isn't what's final, though. Concurrent transactions sort
//!   on either side of it, and floors from different memberships aren't
//!   even comparable. What's final is the longest prefix of the timeline
//!   inside the floor, and those prefixes form a chain across devices
//!   whatever decisions each one happened to learn
//!   (`group/settling.rs`). It takes three promises to make that true:
//!   contributors acknowledge each configuration before their reports
//!   count toward it (`Decision::floor`), newcomers catch up before
//!   their first write (`group/admission.rs`), and a removed device's
//!   writes count only if a contributor vouches for them
//!   (`group/removal.rs`).
//! - Two reads fall out of that split. Settled state is the same on every
//!   device and only moves forward. The frontier on top of it is the
//!   latest this device knows, and may still be reordered
//!   ([`Query::committed`]).
//! - Many models per agreement: one clock spans them all, a model is
//!   just a namespace in that history, and one transaction can change
//!   several.
//! - Rules as declarations. A model says what each operation writes and
//!   what it requires of its key. Everything else (who wins, what's
//!   pruned, what readers see, what settles) follows from the timeline,
//!   in code that never looks inside an operation (`group/verdicts.rs`).
//!   Last-writer-wins is "requires nothing". First-writer-wins is
//!   "requires the key to be vacant" (`model/claims.rs`).
//! - Everything that travels is signed by whoever said it: transactions,
//!   progress reports, proposals, attestations. A decision travels with
//!   the attestations that prove it (`agreement/certificate.rs`). So sync
//!   and agreement both work over chains of devices, not just a full
//!   mesh, and a device with no write access can relay all of it without
//!   being trusted with any of it.
//! - A proof of a decision is only checkable by someone who knows the
//!   contributors it counts. Keeping one proof per change of
//!   configuration is enough to walk anyone up from the genesis
//!   (`a_replica_far_behind_follows_each_change_of_configuration`).
//! - Reads bypass the state machine. A change is written in the batch
//!   that finds it live, and writes precede every other action, so
//!   storage is current once a batch's actions are performed. A
//!   [`Query`] is just a key range and a decoder, and [`Changed`] says
//!   when to run it again.
//! - Declarations are also what's worth remembering. Verdicts never
//!   read a value, so the log keeps an outline of each unsettled
//!   transaction and leaves the transaction in storage. It comes back a
//!   page at a time, to be sent, settled, or shown
//!   (`group/bodies.rs`).
//! - Settling bounds hydration. Only protocol state loads at startup, a
//!   page at a time, and the log in it holds only what hasn't settled
//!   yet (`storage/keys.rs`).
//! - The timeline and sans-IO. A write is stored as a version of its key
//!   under its transaction's stamp, so storage's key order is the timeline
//!   and integrating never reads (`model/kv.rs`,
//!   `an_older_write_still_loses_after_a_restart`). The first attempt kept
//!   one entry per key and compared stamps in memory, which made every
//!   change a read-modify-write, and every read a round trip through the
//!   application. The cost moved to reads, which skip a key's older
//!   versions, and to storage, which keeps them until they settle into
//!   one value per key. A settled removal leaves nothing, so there are no
//!   tombstones to collect.
//!
//! **What fights**
//!
//! - Removal and finality. RLA gives comparable decisions, not a "first
//!   decision that included this change", so there's no moment at which a
//!   removed device's writes stop counting. Asking who vouched for them
//!   is deterministic, but it costs data: writes nobody took before
//!   hearing of the removal are discarded, and a device that isn't a
//!   contributor and took them anyway has to go back to what's settled
//!   and sync again (`group/removal.rs`).
//! - Reconfiguration and settling. Every membership change holds
//!   settling back until every contributor has acknowledged it, so one
//!   offline contributor stalls committed reads for everyone. Removing
//!   it is the only way out.
//! - Requirements and sans-IO. Blind writes were enough while nothing
//!   depended on what a key held. A requirement does, and what has
//!   settled is only in storage, so the point read came back, and with
//!   it a third verdict: pending. It only costs a round trip the first
//!   time a key is asked about (`group/verdicts.rs`).
//! - An offline contributor and memory. Nothing settles past what it
//!   has, so the frontier grows for as long as it's away. Values stay in
//!   storage, but an outline per transaction is still in memory, and
//!   every verdict in the frontier is worked out again when it changes.
//! - Honest contributors only. Nobody else has to be trusted, but a
//!   lying contributor can report progress it hasn't made, attest to a
//!   view it doesn't hold, or equivocate. Byzantine RLA is a separate
//!   line of research.

mod actor;
mod agreement;
mod clock;
mod encoding;
mod group;
mod io;
mod lattice;
mod log;
mod model;
mod node;
mod storage;
#[cfg(test)]
mod testing;

pub use actor::Identity;
pub use agreement::{AgreementId, Genesis};
pub use io::{Action, Changed, NetworkAction};
pub use model::{ColumnId, ModelId, Query, Row, Table, Value};
pub use node::{Batch, Node, TransactionBuilder};
pub use storage::{StorageAction, StorageWrite};
