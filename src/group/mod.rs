//! A group: one agreement and everything it owns on this device.
//!
//! This is where the independent pieces meet. Each one stays ignorant of
//! the others, and the group is the only place that knows how they connect:
//!
//! - the **agreement** decides who may write, and proves the compaction
//!   floor;
//! - the **history** (the clock and the transaction log) orders and
//!   deduplicates transactions;
//! - the **models** apply them, once the entries they change are loaded.
//!
//! The life of a batch, from the group's point of view:
//!
//! 1. [`write`](Group::write) and [`receive`](Group::receive) take in local
//!    and remote changes, and [`loaded`](Group::loaded) takes in model
//!    entries from storage. Changes only queue up at this point.
//! 2. [`settle`](Group::settle) runs once all events are in. It merges
//!    snapshots, learns what it can, integrates queued transactions, reports
//!    progress, compacts, and persists what changed. Anything waiting on
//!    model entries asks for them and waits for a later batch.
//! 3. [`outgoing`](Group::outgoing) says what each peer is missing.

mod compaction;
mod history;
mod models;
mod persistence;
mod snapshot;
mod sync;

use std::collections::{BTreeMap, BTreeSet};

use history::History;
use models::Models;
pub(crate) use persistence::Restored;
use snapshot::{Snapshot, Snapshots};
use sync::PeerSync;
pub(crate) use sync::{GroupMessage, VerifiedParts};

use crate::actor::{Identity, Peer, Signed};
use crate::agreement::{Agreement, AgreementId, Change, Genesis};
use crate::clock::Progress;
use crate::model::{Entry, ModelId, Operation, Transaction};
use crate::node::Outbox;

#[derive(Debug)]
pub(crate) struct Group {
    id: AgreementId,
    me: Peer,
    agreement: Agreement,
    history: History,
    models: Models,
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
    pub(crate) fn models(&self) -> &Models {
        &self.models
    }

    #[cfg(test)]
    pub(crate) fn history(&self) -> &History {
        &self.history
    }

    /// Commits a local transaction. It's signed now, so the stored and sent
    /// bytes are final, and it's persisted right away so it survives a
    /// restart even if this device can't integrate it yet.
    pub(crate) fn write(
        &mut self,
        identity: &Identity,
        model: ModelId,
        operations: Vec<Operation>,
        outbox: &mut Outbox,
    ) {
        let transaction = self.history.author(self.id, model, operations);
        let signed = Signed::sign(identity, transaction);
        persistence::save_transaction(self.id, signed.signed(), outbox);

        self.history.offer(signed);
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
            let snapshot = snapshot.clone();
            (self.snapshots).arrived(snapshot, self.id, &mut self.models, outbox);
        }

        for transaction in signed.transactions {
            self.history.offer(transaction);
        }
    }

    /// Storage answered for a model entry this group asked for.
    pub(crate) fn loaded(&mut self, model: ModelId, key: Vec<u8>, entry: Option<Entry>) {
        self.models.loaded(model, key, entry);
    }

    /// Our scan of the data range is back, for a snapshot.
    pub(crate) fn scanned(&mut self, entries: &[(Vec<u8>, Vec<u8>)]) {
        self.snapshots.scanned(entries);
    }

    /// Runs after every event in the batch is in. `identity` must be this
    /// device's, for attesting to the settled view.
    pub(crate) fn settle(&mut self, identity: &Identity, outbox: &mut Outbox) {
        for snapshot in self.snapshots.take_loaded(&self.models) {
            self.absorb(&snapshot, outbox);
        }

        // Learning first: a decision may grant write access to transactions
        // that are already waiting.
        self.agreement.try_learn();
        self.integrate(outbox);

        // Contributors report how far they've gotten. That report is what
        // eventually lets everyone compact.
        if self.may_report() {
            let report = Progress::report(self.me, self.history.integrated().clone());
            self.agreement.propose_progress(&report);
        }

        self.agreement.try_learn();
        for (author, through) in compaction::compact(&mut self.history, self.agreement.learned()) {
            persistence::forget_transactions(self.id, author, through, outbox);
        }

        // Last, so it covers everything above. It's saved in the same write
        // as the view, before anything is sent.
        self.agreement.attest(identity);
        self.persisted
            .save(self.id, &self.agreement, &self.history, outbox);
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
        if sync.is_behind(self.history.floor()) {
            self.snapshots.prepare(self.id, &self.history, outbox);
        }

        let snapshot = self.snapshots.prepared();
        sync.outgoing(peer, &self.agreement, &self.history, snapshot)
    }

    /// Whether to propose a new progress report: only as a contributor, and
    /// only once the previous report has been learned.
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
            && learned.progress.of(self.me) == proposed.of(self.me)
    }

    /// Integrates whatever is in order, authorized and loaded. Everything
    /// else that's authorized asks for its entries now, so it can go in a
    /// later batch.
    fn integrate(&mut self, outbox: &mut Outbox) {
        let writers = &self.agreement.learned().configuration;
        for transaction in self.history.waiting_for(writers) {
            let model = transaction.model;
            (self.models).request(self.id, model, touched(transaction), outbox);
        }

        let models = &self.models;
        let loaded =
            |transaction: &Transaction| models.has(transaction.model, touched(transaction));
        for transaction in self.history.integrate_ready(writers, loaded) {
            let author = transaction.author();
            let payload = transaction.payload();
            persistence::save_transaction(self.id, transaction.signed(), outbox);

            let stamp = payload.stamp(author);
            let changed = self.models.apply(payload.model, stamp, &payload.operations);
            let model = payload.model;
            persistence::save_entries(self.id, model, &self.models, &changed, outbox);
        }
    }

    fn absorb(&mut self, snapshot: &Snapshot, outbox: &mut Outbox) {
        for (&model, theirs) in &snapshot.models {
            let changed = self.models.merge(model, theirs);
            persistence::save_entries(self.id, model, &self.models, &changed, outbox);
        }

        let forgotten = self.history.absorb(&snapshot.floor, snapshot.lamport);
        for (author, through) in forgotten {
            persistence::forget_transactions(self.id, author, through, outbox);
        }
    }
}

/// The keys a transaction changes.
fn touched(transaction: &Transaction) -> impl Iterator<Item = &[u8]> {
    transaction.operations.iter().map(Operation::key)
}
