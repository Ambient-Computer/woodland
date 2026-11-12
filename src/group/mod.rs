//! A group: one agreement and everything it owns on this device.
//!
//! This is where the independent pieces meet. Each one stays ignorant of
//! the others, and the group is the only place that knows how they connect:
//!
//! - the **agreement** decides who may write, and proves the compaction
//!   floor;
//! - the **log** (transactions and their clock) orders and deduplicates
//!   transactions;
//! - the **models** say what each transaction writes to storage;
//! - **admission** holds this device's writes until it may make them.
//!
//! The life of a batch, from the group's point of view:
//!
//! 1. [`write`](Group::write) and [`receive`](Group::receive) take in local
//!    and remote changes. Transactions only queue up at this point.
//! 2. [`settle`](Group::settle) runs once all events are in. It learns what
//!    it can, integrates queued transactions, signs held writes if this
//!    device was admitted, reports progress, compacts, and persists what
//!    changed.
//! 3. [`outgoing`](Group::outgoing) says what each peer is missing.

mod admission;
mod compaction;
mod persistence;
mod snapshot;
mod sync;

use std::collections::{BTreeMap, BTreeSet};

use admission::Held;
pub(crate) use persistence::Restored;
use snapshot::{Snapshot, Snapshots};
use sync::PeerSync;
pub(crate) use sync::{GroupMessage, VerifiedParts};

use crate::actor::{Identity, Peer, Signed};
use crate::agreement::{Agreement, AgreementId, Change, Genesis};
use crate::clock::Progress;
use crate::log::Log;
use crate::model::{ModelId, Operation};
use crate::node::Outbox;

#[derive(Debug)]
pub(crate) struct Group {
    id: AgreementId,
    me: Peer,
    agreement: Agreement,
    log: Log,
    /// Writes made before this device was admitted.
    held: Held,
    snapshots: Snapshots,
    /// Connected peers subscribed to this agreement.
    peers: BTreeMap<Peer, PeerSync>,
    /// What's in storage, so `settle` only writes what changed.
    persisted: persistence::Persisted,
}

impl Group {
    /// Starts replicating an agreement from its genesis.
    pub(crate) fn join(genesis: &Genesis, me: Peer, outbox: &mut Outbox) -> Self {
        persistence::save_genesis(genesis, outbox);

        Self::from(Restored::fresh(genesis, me))
    }

    pub(crate) fn id(&self) -> AgreementId {
        self.id
    }

    pub(crate) fn agreement(&self) -> &Agreement {
        &self.agreement
    }

    #[cfg(test)]
    pub(crate) fn log(&self) -> &Log {
        &self.log
    }

    /// Commits a local transaction, or holds it until this device is
    /// admitted. Either way it's persisted right away, so it survives a
    /// restart.
    pub(crate) fn write(
        &mut self,
        identity: &Identity,
        model: ModelId,
        operations: Vec<Operation>,
        outbox: &mut Outbox,
    ) {
        if self.log.admitted() {
            self.sign(identity, model, operations, outbox);
        } else {
            self.held.hold(self.id, (model, operations), outbox);
        }
    }

    pub(crate) fn propose(&mut self, change: Change) {
        self.agreement.propose_change(change);
    }

    /// Takes in what a peer sent. Its signed parts arrive already verified.
    pub(crate) fn receive(
        &mut self,
        from: Peer,
        message: &GroupMessage,
        signed: VerifiedParts,
        outbox: &mut Outbox,
    ) {
        if let Some(view) = &message.view {
            self.agreement.receive(view);
        }

        for attestation in signed.attestations {
            self.agreement.hear(attestation);
        }

        if let Some(clock) = &message.clock
            && let Some(peer) = self.peers.get_mut(&from)
        {
            peer.heard_clock(clock);
        }

        if let Some(snapshot) = &message.snapshot {
            self.absorb(snapshot, outbox);
        }

        for transaction in signed.transactions {
            self.log.offer(transaction);
        }
    }

    /// Our scan of the data range is back, for a snapshot.
    pub(crate) fn scanned(&mut self, entries: &[(Vec<u8>, Vec<u8>)]) {
        self.snapshots.scanned(entries);
    }

    /// Runs after every event in the batch is in. `identity` must be this
    /// device's, for attesting to the settled view.
    pub(crate) fn settle(&mut self, identity: &Identity, outbox: &mut Outbox) {
        // Learning first: a decision may grant write access to transactions
        // that are already waiting.
        self.agreement.try_learn();
        self.integrate(outbox);
        self.admit(identity, outbox);

        // Contributors report how far they've gotten. That report is what
        // eventually lets everyone compact.
        if self.may_report() {
            let report = Progress::report(self.me, self.log.integrated().clone());
            self.agreement.propose_progress(&report);
        }

        self.agreement.try_learn();
        for (author, through) in compaction::compact(&mut self.log, self.agreement.learned()) {
            persistence::forget_transactions(self.id, author, through, outbox);
        }

        // Last, so it covers everything above. It's saved in the same write
        // as the view, before anything is sent.
        self.agreement.attest(identity);
        self.persisted
            .save(self.id, &self.agreement, &self.log, outbox);
    }

    /// Tracks exactly the peers that are connected and subscribed.
    pub(crate) fn subscribe_only(&mut self, subscribers: &BTreeSet<Peer>) {
        self.peers.retain(|peer, _| subscribers.contains(peer));
        for peer in subscribers {
            self.peers.entry(*peer).or_default();
        }
    }

    /// What `peer` is missing. A peer behind our floor needs a snapshot,
    /// which may have to be read from storage first.
    pub(crate) fn outgoing(&mut self, peer: Peer, outbox: &mut Outbox) -> Option<GroupMessage> {
        let sync = self.peers.get_mut(&peer)?;
        if sync.is_behind(self.log.floor()) {
            self.snapshots.prepare(self.id, &self.log, outbox);
        }

        let snapshot = self.snapshots.prepared();
        sync.outgoing(peer, &self.agreement, &self.log, snapshot)
    }

    /// Whether to propose a new progress report: only as a contributor
    /// that has been admitted, and only once the previous report has been
    /// learned.
    ///
    /// A newcomer's report is a promise that everything it writes builds on
    /// it, and other newcomers catch up to it. Until it's admitted, it
    /// hasn't caught up to anything worth promising.
    ///
    /// Learning needs a quorum to hold the same view at once. Every report
    /// changes the view, so contributors that report on every batch keep
    /// moving the target, and with steady writes nothing is ever learned
    /// (see `steady_writing_still_compacts`). Holding each contributor to
    /// one report in flight bounds the churn, and the floor trails by about
    /// one round.
    fn may_report(&self) -> bool {
        let learned = self.agreement.learned();
        let proposed = &self.agreement.view().candidate;

        learned.configuration.is_member(self.me)
            && self.log.admitted()
            && learned.progress.of(self.me) == proposed.of(self.me)
    }

    /// Admits this device once it has learned it's a contributor and caught
    /// up, then signs the writes it was holding. A device that learns it
    /// was removed before it was ever admitted drops them.
    fn admit(&mut self, identity: &Identity, outbox: &mut Outbox) {
        let learned = self.agreement.learned();
        if !self.log.admitted() {
            if admission::caught_up(learned, self.me, self.log.integrated()) {
                self.log.admit();
            } else if learned.configuration.was_ever_member(self.me)
                && !learned.configuration.is_member(self.me)
            {
                let dropped = self.held.release(self.id, outbox).len();
                if dropped > 0 {
                    tracing::warn!(dropped, "removed before admission, dropped held writes");
                }
            }
        }

        if self.log.admitted() {
            let held = self.held.release(self.id, outbox);
            let any = !held.is_empty();
            for (model, operations) in held {
                self.sign(identity, model, operations, outbox);
            }

            if any {
                self.integrate(outbox);
            }
        }
    }

    /// Signs a transaction on top of everything integrated here. The stored
    /// and sent bytes are final from now on.
    fn sign(
        &mut self,
        identity: &Identity,
        model: ModelId,
        operations: Vec<Operation>,
        outbox: &mut Outbox,
    ) {
        let transaction = self.log.author(self.id, model, operations);
        let signed = Signed::sign(identity, transaction);
        persistence::save_transaction(self.id, signed.signed(), outbox);

        self.log.offer(signed);
    }

    /// Integrates whatever is in order and authorized, and writes what it
    /// leaves behind. Nothing is read first: a transaction's versions go
    /// wherever its stamp puts them in the timeline.
    fn integrate(&mut self, outbox: &mut Outbox) {
        let writers = &self.agreement.learned().configuration;
        for transaction in self.log.integrate_ready(writers) {
            let author = transaction.author();
            let payload = transaction.payload();
            persistence::save_transaction(self.id, transaction.signed(), outbox);

            let (model, stamp) = (payload.model, payload.stamp(author));
            persistence::save_operations(self.id, model, stamp, &payload.operations, outbox);
        }
    }

    /// Takes on a peer's snapshot. Its versions merge with ours by union.
    fn absorb(&mut self, snapshot: &Snapshot, outbox: &mut Outbox) {
        for it in &snapshot.versions {
            let value = it.value.clone();
            persistence::save_version(self.id, it.model, &it.key, it.stamp, value, outbox);
        }

        let forgotten = self.log.absorb(&snapshot.floor, snapshot.lamport);
        for (author, through) in forgotten {
            persistence::forget_transactions(self.id, author, through, outbox);
        }
    }
}
