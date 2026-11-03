//! A group: one agreement and everything it owns on this device.
//!
//! This is where the independent pieces meet. Each one stays ignorant of
//! the others, and the group is the only place that knows how they connect:
//!
//! - the **agreement** decides who may write, and proves the compaction
//!   floor;
//! - the **history** (the clock and the transaction log) orders and
//!   deduplicates transactions;
//! - the **models** apply them.
//!
//! The life of a batch, from the group's point of view:
//!
//! 1. [`write`](Group::write) and [`receive`](Group::receive) take in local
//!    and remote changes. Transactions only queue up at this point.
//! 2. [`settle`](Group::settle) runs once all events are in. It learns what
//!    it can, integrates queued transactions, reports progress, compacts,
//!    and persists what changed.
//! 3. [`outgoing`](Group::outgoing) says what each peer is missing.

mod compaction;
mod history;
mod persistence;
mod sync;

use std::collections::{BTreeMap, BTreeSet};

use history::History;
pub(crate) use persistence::Restored;
use sync::PeerSync;
pub(crate) use sync::{GroupMessage, Snapshot};

use crate::actor::{Identity, Peer, Signed, Verified};
use crate::agreement::{Agreement, AgreementId, Change, Genesis, View};
use crate::clock::Progress;
use crate::model::{KvModel, ModelId, Operation, Transaction};
use crate::node::Outbox;

#[derive(Debug)]
pub(crate) struct Group {
    id: AgreementId,
    me: Peer,
    agreement: Agreement,
    history: History,
    models: BTreeMap<ModelId, KvModel>,
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

    pub(crate) fn model(&self, model: ModelId) -> Option<&KvModel> {
        self.models.get(&model)
    }

    #[cfg(test)]
    pub(crate) fn models(&self) -> &BTreeMap<ModelId, KvModel> {
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

    /// Takes in what a peer sent. Transactions arrive already verified.
    pub(crate) fn receive(
        &mut self,
        from: Peer,
        message: &GroupMessage,
        transactions: Vec<Verified<Transaction>>,
        outbox: &mut Outbox,
    ) {
        if let Some(view) = &message.view {
            self.agreement.receive(from, view);
        }

        if let Some(clock) = &message.clock
            && let Some(peer) = self.peers.get_mut(&from)
        {
            peer.heard_clock(clock);
        }

        if let Some(snapshot) = &message.snapshot {
            self.absorb(snapshot, outbox);
        }

        for transaction in transactions {
            self.history.offer(transaction);
        }
    }

    /// Runs after every event in the batch is in.
    pub(crate) fn settle(&mut self, outbox: &mut Outbox) {
        // Learning first: a decision may grant write access to transactions
        // that are already waiting.
        self.agreement.try_learn();
        self.integrate(outbox);

        // Contributors report how far they've gotten. That report is what
        // eventually lets everyone compact.
        let learned = &self.agreement.learned().configuration;
        if learned.is_member(self.me) {
            let report = Progress::report(self.me, self.history.integrated().clone());
            self.agreement.propose_progress(&report);
        }

        self.agreement.try_learn();
        for (author, through) in compaction::compact(&mut self.history, self.agreement.learned()) {
            persistence::forget_transactions(self.id, author, through, outbox);
        }

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

    pub(crate) fn outgoing(&mut self, peer: Peer) -> Option<GroupMessage> {
        let view: &View = self.agreement.view();

        self.peers
            .get_mut(&peer)?
            .outgoing(view, &self.history, &self.models)
    }

    fn integrate(&mut self, outbox: &mut Outbox) {
        let writers = &self.agreement.learned().configuration;
        for transaction in self.history.integrate_ready(writers) {
            let author = transaction.author();
            let payload = transaction.payload();
            persistence::save_transaction(self.id, transaction.signed(), outbox);

            let model = self.models.entry(payload.model).or_default();
            let changed = model.apply(payload.stamp(author), &payload.operations);
            persistence::save_entries(self.id, payload.model, model, &changed, outbox);
        }
    }

    fn absorb(&mut self, snapshot: &Snapshot, outbox: &mut Outbox) {
        for (id, theirs) in &snapshot.models {
            let model = self.models.entry(*id).or_default();
            let changed = model.merge(theirs);
            persistence::save_entries(self.id, *id, model, &changed, outbox);
        }

        let forgotten = self.history.absorb(&snapshot.floor, snapshot.lamport);
        for (author, through) in forgotten {
            persistence::forget_transactions(self.id, author, through, outbox);
        }
    }
}
