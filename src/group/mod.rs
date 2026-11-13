//! A group: one agreement and everything it owns on this device.
//!
//! This is where the independent pieces meet. Each one stays ignorant of
//! the others, and the group is the only place that knows how they connect:
//!
//! - the **agreement** decides who may write, and proves how much of the
//!   timeline has settled;
//! - the **log** (transactions and their clock) orders and deduplicates
//!   transactions, and remembers an outline of each;
//! - **bodies** are the few transactions whose contents are in memory,
//!   read back from storage a page at a time;
//! - the **models** say what each transaction writes to storage;
//! - **admission** holds this device's writes until it may make them;
//! - **removal** says which of a removed contributor's writes still count;
//! - **verdicts** say which transactions' requirements hold.
//!
//! The life of a batch, from the group's point of view:
//!
//! 1. [`write`](Group::write) and [`receive`](Group::receive) take in local
//!    and remote changes. Transactions only queue up at this point.
//! 2. [`advance`](Group::advance) runs once all events are in. It learns
//!    what it can, integrates queued transactions, signs held writes if
//!    this device was admitted, reports progress, works out which
//!    transactions happen, settles what's final, and persists what
//!    changed.
//! 3. [`outgoing`](Group::outgoing) says what each peer is missing.

mod admission;
mod bodies;
mod persistence;
mod removal;
mod settling;
mod snapshot;
mod sync;
mod verdicts;

use std::collections::{BTreeMap, BTreeSet};

use admission::Held;
use bodies::{Bodies, PAGE};
pub(crate) use persistence::Restored;
use snapshot::{Snapshot, Snapshots};
use sync::PeerSync;
pub(crate) use sync::{GroupMessage, VerifiedParts};
use verdicts::{Verdict, Verdicts};

use crate::actor::{Identity, Peer, Signed, Verified};
use crate::agreement::{Agreement, AgreementId, Change, Genesis};
use crate::io::Changed;
use crate::lattice::Lattice;
use crate::log::{Log, Outline, Transaction};
use crate::model::{Declaration, Effect, Kind, ModelId, Operation};
use crate::node::Outbox;

#[derive(Debug)]
pub(crate) struct Group {
    id: AgreementId,
    me: Peer,
    agreement: Agreement,
    log: Log,
    /// Which transactions in the frontier happen.
    verdicts: Verdicts,
    bodies: Bodies,
    /// Live transactions whose versions aren't in storage yet, and
    /// whether storage has been asked for them.
    unwritten: BTreeMap<(Peer, u64), bool>,
    /// The start of the frontier, while storage reads it back to settle
    /// it.
    settling: Option<Vec<Outline>>,
    /// Writes made before this device was admitted.
    held: Held,
    snapshots: Snapshots,
    /// Connected peers subscribed to this agreement.
    peers: BTreeMap<Peer, PeerSync>,
    /// What's in storage, so `advance` only writes what changed.
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
        operations: Vec<Operation>,
        outbox: &mut Outbox,
    ) {
        let configuration = &self.agreement.learned().configuration;
        if configuration.removed().any(|peer| peer == self.me) {
            tracing::warn!("removed from the agreement, dropped a write");
        } else if self.log.admitted() {
            self.sign(identity, operations, outbox);
        } else {
            self.held.hold(self.id, operations, outbox);
        }
    }

    pub(crate) fn propose(&mut self, identity: &Identity, change: Change) {
        if !self.agreement.propose_change(identity, change) {
            tracing::warn!("not a contributor, dropped a membership change");
        }
    }

    /// Takes in what a peer sent. Its signed parts arrive already verified.
    pub(crate) fn receive(
        &mut self,
        from: Peer,
        message: &GroupMessage,
        signed: VerifiedParts,
        outbox: &mut Outbox,
    ) {
        if let Some(gossip) = &message.gossip {
            let (proposals, reports) = (signed.proposals, signed.reports);
            let decisions = gossip.decisions.clone();
            self.agreement.receive(decisions, proposals, reports);
        }

        for attestation in signed.attestations {
            self.agreement.hear(attestation);
        }

        if let Some(peer) = self.peers.get_mut(&from) {
            if message.rewound {
                peer.heard_rewind();
            }

            if let Some(clock) = &message.clock {
                peer.heard_clock(clock);
            }
        }

        if let Some(snapshot) = &message.snapshot {
            self.absorb(snapshot, outbox);
        }

        for transaction in signed.transactions {
            self.log.offer(transaction);
        }
    }

    /// Storage answered: a transaction we asked for, or that it's gone.
    pub(crate) fn fetched(&mut self, key: (Peer, u64), stored: Option<Verified<Transaction>>) {
        self.bodies.read(key, stored);
    }

    /// The batch is over, and so is whatever was in hand for it.
    pub(crate) fn finish(&mut self) {
        self.bodies.clear();
    }

    /// Our scan of the settled range is back, for a snapshot.
    pub(crate) fn scanned(&mut self, entries: &[(Vec<u8>, Vec<u8>)]) {
        self.snapshots.scanned(entries);
    }

    /// A read of a settled value is back, for a verdict.
    pub(crate) fn got(&mut self, model: ModelId, key: Vec<u8>, holds: bool) {
        self.verdicts.got(model, key, holds);
    }

    /// Runs after every event in the batch is in. `identity` must be this
    /// device's, for attesting to the view it ends up with.
    pub(crate) fn advance(&mut self, identity: &Identity, outbox: &mut Outbox) {
        // Learning first: a decision may grant write access to transactions
        // that are already waiting.
        self.agreement.try_learn(identity);
        self.rewind(outbox);
        self.integrate(outbox);
        self.admit(identity, outbox);

        // Contributors report how far they've gotten, and which
        // configuration they knew of. That report is what eventually lets
        // everyone settle.
        if self.may_report() {
            let integrated = self.log.integrated().clone();
            self.agreement.report(identity, integrated);
        }

        self.agreement.try_learn(identity);
        self.judge(outbox);
        self.settle(outbox);

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

    /// What `peer` is missing. A peer that lacks what we've settled needs
    /// a snapshot, which may have to be read from storage first.
    pub(crate) fn outgoing(&mut self, peer: Peer, outbox: &mut Outbox) -> Option<GroupMessage> {
        let sync = self.peers.get_mut(&peer)?;
        if sync.is_behind(self.log.settled()) {
            self.snapshots.prepare(self.id, &self.log, outbox);
        }

        let snapshot = self.snapshots.prepared();
        let (agreement, log, bodies) = (&self.agreement, &self.log, &mut self.bodies);
        sync.outgoing(peer, agreement, log, snapshot, bodies, outbox)
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
    /// (see `steady_writing_still_settles`). Holding each contributor to
    /// one report in flight bounds the churn, and the floor trails by about
    /// one round.
    fn may_report(&self) -> bool {
        let learned = self.agreement.learned();
        let view = self.agreement.view();

        learned.configuration.is_member(self.me)
            && self.log.admitted()
            && learned.progress.of(self.me) == view.candidate.of(self.me)
            && learned.acknowledged.get(&self.me) == view.acknowledged.get(&self.me)
    }

    /// Admits this device once it has learned it's a contributor and caught
    /// up, then signs the writes it was holding. A device that learns it
    /// was removed before it was ever admitted drops them.
    fn admit(&mut self, identity: &Identity, outbox: &mut Outbox) {
        let learned = self.agreement.learned();
        if !self.log.admitted() {
            let integrated = self.log.integrated();
            if admission::caught_up(learned, self.me, integrated)
                && !removal::holds_unvouched(learned, integrated)
            {
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
            for operations in held {
                self.sign(identity, operations, outbox);
            }

            if any {
                self.integrate(outbox);
            }
        }
    }

    /// Signs a transaction on top of everything integrated here. The stored
    /// and sent bytes are final from now on.
    fn sign(&mut self, identity: &Identity, operations: Vec<Operation>, outbox: &mut Outbox) {
        let transaction = self.log.author(self.id, operations);
        let signed = Signed::sign(identity, transaction);
        persistence::save_transaction(self.id, signed.signed(), outbox);

        self.log.offer(signed);
    }

    /// Goes back to what has settled if anything integrated since includes
    /// transactions the contributors agreed to discard (see `removal`).
    /// Peers send the rest again, without them.
    fn rewind(&mut self, outbox: &mut Outbox) {
        let learned = self.agreement.learned();
        let (me, admitted) = (self.me, self.log.admitted());
        self.log
            .forget_waiting(|author, seq| removal::abandoned(learned, me, author, seq));

        if !removal::must_rewind(learned, me, admitted, self.log.integrated()) {
            return;
        }

        tracing::info!("rewinding: holding transactions nobody vouched for");
        self.log.rewind();
        self.unwritten.clear();
        self.settling = None;
        persistence::forget_frontier(self.id, outbox);
        outbox.changed(Changed::Everything { agreement: self.id });
        for peer in self.peers.values_mut() {
            peer.rewound();
        }
    }

    /// Integrates whatever is in order and counts.
    fn integrate(&mut self, outbox: &mut Outbox) {
        let learned = self.agreement.learned();
        let counts = |author, seq| removal::counts(learned, author, seq);
        for transaction in self.log.integrate_ready(counts) {
            persistence::save_transaction(self.id, transaction.signed(), outbox);
            self.bodies.hold(transaction);
        }
    }

    /// Works out which transactions in the frontier happen, and makes
    /// storage hold the writes of exactly those. A live transaction's
    /// versions go wherever its stamp puts them in the timeline, without
    /// reading what's there.
    fn judge(&mut self, outbox: &mut Outbox) {
        let frontier = self.log.frontier();
        let dropped = &self.log.cut().dropped;
        for (outline, verdict) in self.verdicts.judge(self.id, dropped, &frontier, outbox) {
            match verdict {
                Verdict::Live => {
                    self.unwritten.insert(outline.key(), false);
                }

                Verdict::Pruned => {
                    let (stamp, declared) = (outline.stamp(), &outline.declared);
                    self.unwritten.remove(&outline.key());
                    persistence::forget_operations(self.id, stamp, declared, outbox);
                    Self::notify_shown(self.id, declared, outbox);
                }

                Verdict::Pending => {}
            }
        }

        self.show(outbox);
    }

    /// Writes the versions of live transactions that storage doesn't
    /// have yet. One integrated in this batch is in hand. One that turned
    /// out to be live later has to be read back first.
    fn show(&mut self, outbox: &mut Outbox) {
        for (key, asked) in std::mem::take(&mut self.unwritten) {
            if !self.log.holds(key.0, key.1) {
                continue;
            }

            if let Some(transaction) = self.bodies.get(key) {
                let payload = transaction.payload();
                let stamp = payload.stamp(transaction.author());
                persistence::save_operations(self.id, stamp, &payload.operations, outbox);

                let operations = payload.operations.iter();
                let declared: Vec<_> = operations.flat_map(Operation::declares).collect();
                Self::notify_shown(self.id, &declared, outbox);
            } else if self.bodies.answered(key) {
                tracing::error!("a transaction in the frontier is missing from storage");
            } else {
                if !asked {
                    self.bodies.ask(key, outbox);
                }

                self.unwritten.insert(key, true);
            }
        }
    }

    /// Tells whoever is watching that what optimistic readers see at a
    /// transaction's keys may be different.
    fn notify_shown(agreement: AgreementId, declared: &[Declaration], outbox: &mut Outbox) {
        let shown = |operation: &&Declaration| {
            let kind = operation.model.kind();

            kind.is_some_and(Kind::shows_unsettled)
        };

        for operation in declared.iter().filter(shown) {
            Self::notify(agreement, operation, false, outbox);
        }
    }

    /// Tells whoever is watching that an operation's key changed, or its
    /// whole model, if it dropped one.
    fn notify(
        agreement: AgreementId,
        operation: &Declaration,
        committed: bool,
        outbox: &mut Outbox,
    ) {
        let change = match &operation.effect {
            Effect::Write { key, .. } => Changed::Key {
                agreement,
                model: operation.model,
                key: key.clone(),
                committed,
            },
            Effect::Drop => Changed::Everything { agreement },
            Effect::Require { .. } => return,
        };

        outbox.changed(change);
    }

    /// Settles what agreement proves is final, a page at a time: the
    /// live transactions among them have to be read back from storage
    /// first, and the answer comes in a later batch.
    fn settle(&mut self, outbox: &mut Outbox) {
        loop {
            let verdicts = &self.verdicts;
            let ready = |outline: &Outline| !verdicts.is_pending(outline);
            let learned = self.agreement.learned();
            let next = settling::settleable(&self.log, learned, ready, PAGE);
            let needed = |outline: &&Outline| verdicts.is_live(outline);
            let in_hand = |page: &[Outline]| {
                let mut needed = page.iter().filter(needed);

                needed.all(|it| self.bodies.get(it.key()).is_some())
            };

            if let Some(page) = self.settling.take() {
                // Something else may have changed the frontier since.
                if !next.starts_with(&page) {
                    continue;
                }

                if !in_hand(&page) {
                    let mut needed = page.iter().filter(needed);
                    if needed.all(|it| self.bodies.answered(it.key())) {
                        tracing::error!("a transaction in the frontier is missing from storage");
                    }

                    self.settling = Some(page);
                    return;
                }

                self.fold(&page, outbox);
            } else if next.is_empty() {
                return;
            } else if in_hand(&next) {
                self.fold(&next, outbox);
            } else {
                for outline in next.iter().filter(needed) {
                    self.bodies.ask(outline.key(), outbox);
                }

                self.settling = Some(next);
                return;
            }
        }
    }

    /// Settles the start of the frontier. A live transaction's writes
    /// become settled values, and a model it drops is emptied. A pruned
    /// one just leaves.
    fn fold(&mut self, page: &[Outline], outbox: &mut Outbox) {
        self.log.settle(page);
        for outline in page {
            let live = self.verdicts.is_live(outline);
            if live && let Some(transaction) = self.bodies.get(outline.key()) {
                let operations = &transaction.payload().operations;
                persistence::fold_operations(self.id, operations, outbox);
                for operation in &outline.declared {
                    match &operation.effect {
                        Effect::Write { key, leaves_value } => {
                            self.verdicts.folded(operation.model, key, *leaves_value);
                        }
                        Effect::Drop => self.log.dropped(operation.model),
                        Effect::Require { .. } => {}
                    }

                    Self::notify(self.id, operation, true, outbox);
                }
            }

            self.unwritten.remove(&outline.key());
            persistence::forget_transaction(self.id, outline, outbox);
        }
    }

    /// Takes on a peer's snapshot, if it has settled more than we have.
    /// Its state replaces ours: both are prefixes of one timeline (see
    /// `settling`), so the longer one already contains the shorter.
    fn absorb(&mut self, snapshot: &Snapshot, outbox: &mut Outbox) {
        let (ours, theirs) = (self.log.settled(), &snapshot.cut.settled);
        if theirs.leq(ours) {
            return;
        }

        if !ours.leq(theirs) {
            tracing::warn!("ignored a snapshot that settled a different history");
            return;
        }

        persistence::save_snapshot(self.id, snapshot, outbox);
        outbox.changed(Changed::Everything { agreement: self.id });
        self.verdicts.absorbed();
        for outline in self.log.absorb(&snapshot.cut) {
            self.unwritten.remove(&outline.key());
            persistence::forget_transaction(self.id, &outline, outbox);
        }
    }
}
