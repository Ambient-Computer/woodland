//! The log: every transaction a device has integrated, and the clock that
//! says which ones those are.
//!
//! # From a tree to a timeline
//!
//! Devices write without asking anyone, so transactions don't arrive as a
//! list. Each one names what it was written on top of (`after`), and two
//! that don't name each other are concurrent: together they form a tree
//! that branches wherever devices wrote at the same time.
//!
//! Deriving state needs a list. The **timeline** is that tree flattened,
//! by sorting on each transaction's [`Stamp`]. A stamp's Lamport counter
//! is greater than that of everything the transaction was written on top
//! of, so the timeline never puts an effect before its cause, and
//! concurrent transactions fall in an order that's arbitrary but the same
//! on every device. Models derive their state from that order. The log
//! doesn't know what's inside an operation.
//!
//! # What the log guarantees
//!
//! - **Causal delivery.** A transaction is integrated only after
//!   everything in its `after`, and after its author's earlier ones. So
//!   what a device has integrated is always a history its authors could
//!   have seen, and a [`VersionVector`] is enough to name it.
//! - **Only transactions that count.** The caller says which do. Others
//!   wait until they count.
//! - **Once each.** A transaction already integrated is ignored.
//!
//! It doesn't guarantee arrival in timeline order. A device that was
//! offline delivers transactions that belong in the middle of everyone
//! else's timeline, and models have to cope with that.
//!
//! [`Stamp`]: crate::clock::Stamp

mod transaction;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::actor::{Peer, Signed, Verified};
use crate::agreement::AgreementId;
use crate::clock::VersionVector;
use crate::lattice::Lattice;
use crate::model::{ModelId, Operation};
pub(crate) use transaction::Transaction;

/// One log on this device.
///
/// Invariant: `transactions` holds exactly those integrated and not yet
/// settled: the frontier. A settled transaction lives on only in what the
/// models derived from it, whether it settled here or arrived in a
/// snapshot.
#[derive(Debug, Default)]
pub(crate) struct Log {
    clock: Clock,
    /// The highest sequence number this device has authored.
    authored: u64,
    transactions: BTreeMap<(Peer, u64), Verified<Transaction>>,
    /// Verified transactions that can't be integrated yet: there's a gap
    /// before them in their author's history, something they were written
    /// on top of is missing, or their author doesn't have write access yet.
    /// They're re-checked whenever any of those might change.
    waiting: BTreeMap<(Peer, u64), Verified<Transaction>>,
}

/// The parts of a [`Log`] that are persisted as one record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Clock {
    /// Per author, how far this device has integrated.
    pub(crate) integrated: VersionVector,
    /// The greatest Lamport counter seen. New transactions go above it.
    pub(crate) lamport: u64,
    /// How much of the timeline has settled here.
    pub(crate) cut: Cut,
    /// Whether this device may write (see `group::admission`). Once true,
    /// it stays true.
    pub(crate) admitted: bool,
}

/// A settled prefix of the timeline (see `group::settling`). Every
/// device's cut is a prefix of the same timeline, so any two are
/// comparable.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Cut {
    /// Per author, the transactions in the prefix.
    pub(crate) settled: VersionVector,
    /// The greatest Lamport counter among them. A device that takes the
    /// prefix from a snapshot never sees them, and still has to write
    /// above them.
    pub(crate) lamport: u64,
}

impl Log {
    /// Rebuilds from storage. Stored transactions beyond the clock are this
    /// device's own, written but not integrated before it stopped.
    pub(crate) fn restore(clock: Clock, stored: Vec<Verified<Transaction>>, me: Peer) -> Self {
        let mut log = Self {
            authored: clock.integrated.get(me),
            clock,
            ..Self::default()
        };

        for transaction in stored {
            let author = transaction.author();
            let seq = transaction.payload().seq;
            if author == me {
                log.authored = log.authored.max(seq);
            }

            let destination = if log.clock.integrated.contains(author, seq) {
                &mut log.transactions
            } else {
                &mut log.waiting
            };
            destination.insert((author, seq), transaction);
        }

        log
    }

    pub(crate) fn clock(&self) -> &Clock {
        &self.clock
    }

    pub(crate) fn integrated(&self) -> &VersionVector {
        &self.clock.integrated
    }

    pub(crate) fn cut(&self) -> &Cut {
        &self.clock.cut
    }

    pub(crate) fn settled(&self) -> &VersionVector {
        &self.clock.cut.settled
    }

    pub(crate) fn admitted(&self) -> bool {
        self.clock.admitted
    }

    /// This device may write from now on.
    pub(crate) fn admit(&mut self) {
        self.clock.admitted = true;
    }

    /// The next transaction this device will author. It's written on top of
    /// everything integrated here, and takes the next sequence number and a
    /// Lamport counter above everything seen, so it wins over every write
    /// this device knows about.
    pub(crate) fn author(
        &mut self,
        agreement: AgreementId,
        model: ModelId,
        operations: Vec<Operation>,
    ) -> Transaction {
        self.authored += 1;
        self.clock.lamport += 1;

        Transaction {
            agreement,
            model,
            seq: self.authored,
            after: self.clock.integrated.clone(),
            lamport: self.clock.lamport,
            operations,
        }
    }

    /// Accepts a verified transaction. It waits until it can be integrated.
    ///
    /// An author who signs two different transactions with the same `seq`
    /// (equivocation) splits replicas: each keeps whichever arrived first.
    /// Like the agreement, this assumes crash faults, not lying authors.
    pub(crate) fn offer(&mut self, transaction: Verified<Transaction>) {
        let key = (transaction.author(), transaction.payload().seq);
        if !self.clock.integrated.contains(key.0, key.1) {
            self.waiting.entry(key).or_insert(transaction);
        }
    }

    /// Integrates every waiting transaction that's now in order, follows
    /// only what's integrated, and that `counts` (by author and sequence
    /// number). Returns them in integration order, for the models.
    pub(crate) fn integrate_ready(
        &mut self,
        counts: impl Fn(Peer, u64) -> bool,
    ) -> Vec<Verified<Transaction>> {
        let mut integrated = Vec::new();
        while let Some(key) = self.next_ready(&counts) {
            if let Some(transaction) = self.waiting.remove(&key) {
                self.clock.integrated.advance(key.0, key.1);
                self.clock.lamport = self.clock.lamport.max(transaction.payload().lamport);
                self.transactions.insert(key, transaction.clone());
                integrated.push(transaction);
            }
        }

        integrated
    }

    /// Transactions a peer that has integrated `theirs` is missing. Only
    /// what's in the log: anything settled must go as a snapshot.
    pub(crate) fn missing_from(&self, theirs: &VersionVector) -> Vec<Signed<Transaction>> {
        self.transactions
            .iter()
            .filter(|&(&(author, seq), _)| !theirs.contains(author, seq))
            .map(|(_, transaction)| transaction.signed().clone())
            .collect()
    }

    /// Settles the longest prefix of the timeline that lies within `floor`
    /// and returns it, in timeline order. Those transactions leave the
    /// log.
    ///
    /// An author's transactions appear in the timeline in the order it
    /// wrote them, so a prefix never skips one, and the cut stays a
    /// version vector.
    pub(crate) fn settle(&mut self, floor: &VersionVector) -> Vec<Verified<Transaction>> {
        let mut timeline: Vec<_> = self.transactions.values().collect();
        timeline.sort_by_key(|it| it.payload().stamp(it.author()));

        let within = |it: &&Verified<Transaction>| floor.contains(it.author(), it.payload().seq);
        let settled: Vec<_> = timeline.into_iter().take_while(within).cloned().collect();
        for transaction in &settled {
            let (author, payload) = (transaction.author(), transaction.payload());
            self.transactions.remove(&(author, payload.seq));
            self.clock.cut.settled.advance(author, payload.seq);
            self.clock.cut.lamport = self.clock.cut.lamport.max(payload.lamport);
        }

        settled
    }

    /// Takes on a peer's cut, which must be ahead of ours: everything in
    /// it is reflected in the settled state that came with it, so this
    /// device has it too. Returns the transactions that were still in the
    /// frontier here and are now settled, in no particular order.
    pub(crate) fn absorb(&mut self, cut: &Cut) -> Vec<Verified<Transaction>> {
        self.clock.integrated.join(&cut.settled);
        self.clock.lamport = self.clock.lamport.max(cut.lamport);
        self.clock.cut = cut.clone();

        let integrated = &self.clock.integrated;
        self.waiting
            .retain(|&(author, seq), _| !integrated.contains(author, seq));

        let settled = |&&(author, seq): &&(Peer, u64)| cut.settled.contains(author, seq);
        let absorbed: Vec<_> = self.transactions.keys().filter(settled).copied().collect();

        absorbed
            .iter()
            .filter_map(|key| self.transactions.remove(key))
            .collect()
    }

    /// Drops waiting transactions that will never be integrated.
    pub(crate) fn forget_waiting(&mut self, abandoned: impl Fn(Peer, u64) -> bool) {
        self.waiting
            .retain(|&(author, seq), _| !abandoned(author, seq));
    }

    /// Goes back to the cut: forgets everything that hasn't settled, to
    /// sync it again. This device's own sequence numbers are never reused,
    /// and its Lamport counter never goes back.
    pub(crate) fn rewind(&mut self) {
        self.clock.integrated = self.clock.cut.settled.clone();
        self.transactions.clear();
        self.waiting.clear();
    }

    fn next_ready(&self, counts: impl Fn(Peer, u64) -> bool) -> Option<(Peer, u64)> {
        let mut waiting = self.waiting.iter();
        let (&key, _) = waiting.find(|&(&(author, seq), transaction)| {
            seq == self.clock.integrated.next(author)
                && transaction.payload().after.leq(&self.clock.integrated)
                && counts(author, seq)
        })?;

        Some(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::Signed;
    use crate::agreement::Genesis;
    use crate::testing::peers::{identity_of, laptop, nas, phone};

    fn genesis() -> Genesis {
        Genesis::new([7; 32], [phone().id(), laptop().id(), nas().id()])
    }

    /// One device's log, and the transactions it has signed.
    struct Device {
        me: Peer,
        log: Log,
    }

    impl Device {
        fn new(me: Peer) -> Self {
            let log = Log::restore(Clock::default(), Vec::new(), me);

            Self { me, log }
        }

        /// Writes a transaction and integrates it locally.
        fn write(&mut self) -> Verified<Transaction> {
            let model = ModelId::new(genesis().id(), [1; 32]);
            let transaction = self.log.author(genesis().id(), model, Vec::new());
            let signed = Signed::sign(&identity_of(self.me), transaction);
            self.receive(&signed);

            signed
        }

        /// Takes in a transaction. Returns what could be integrated, as
        /// `(author, seq)`.
        fn receive(&mut self, transaction: &Verified<Transaction>) -> Vec<(Peer, u64)> {
            self.log.offer(transaction.clone());
            let founders = genesis().configuration();
            let counts = |author, _| founders.is_member(author);
            let integrated = self.log.integrate_ready(counts);

            integrated
                .iter()
                .map(|it| (it.author(), it.payload().seq))
                .collect()
        }
    }

    #[test]
    fn a_transaction_names_what_it_was_written_on_top_of() {
        let (mut phone_device, mut laptop_device) = (Device::new(phone()), Device::new(laptop()));

        let first = phone_device.write();
        laptop_device.receive(&first);
        let reply = laptop_device.write();

        assert_eq!(first.payload().after, VersionVector::default());
        assert_eq!(reply.payload().after, [(phone(), 1)].into_iter().collect());
        assert!(reply.payload().lamport > first.payload().lamport);
    }

    #[test]
    fn a_transaction_waits_for_what_it_was_written_on_top_of() {
        let (mut phone_device, mut laptop_device) = (Device::new(phone()), Device::new(laptop()));
        let mut nas_device = Device::new(nas());

        let first = phone_device.write();
        laptop_device.receive(&first);
        let reply = laptop_device.write();

        // The reply arrives first. Nothing is missing from the laptop's own
        // log, but the NAS hasn't seen what the laptop was replying to.
        assert_eq!(nas_device.receive(&reply), []);
        assert_eq!(nas_device.receive(&first), [(phone(), 1), (laptop(), 1)]);
    }

    #[test]
    fn concurrent_transactions_dont_wait_for_each_other() {
        let (mut phone_device, mut laptop_device) = (Device::new(phone()), Device::new(laptop()));
        let mut nas_device = Device::new(nas());

        let ours = phone_device.write();
        let theirs = laptop_device.write();

        assert_eq!(nas_device.receive(&theirs), [(laptop(), 1)]);
        assert_eq!(nas_device.receive(&ours), [(phone(), 1)]);
    }

    /// The floor says who holds what. It doesn't say what's final: a
    /// transaction inside it can sort after one that's still outside.
    #[test]
    fn settling_stops_at_the_first_transaction_outside_the_floor() {
        let (mut phone_device, mut laptop_device) = (Device::new(phone()), Device::new(laptop()));
        let mut nas_device = Device::new(nas());

        let first = phone_device.write();
        let concurrent = laptop_device.write();
        phone_device.receive(&concurrent);
        let second = phone_device.write();
        for transaction in [&first, &concurrent, &second] {
            nas_device.receive(transaction);
        }

        // Everyone holds the phone's writes. Not the laptop's, which sorts
        // before the phone's second.
        let floor = [(phone(), 2)].into_iter().collect();
        nas_device.log.settle(&floor);
        assert!(nas_device.log.settled().get(phone()) < 2);

        let floor: VersionVector = [(phone(), 2), (laptop(), 1)].into_iter().collect();
        nas_device.log.settle(&floor);
        assert_eq!(nas_device.log.settled(), &floor);
        assert_eq!(nas_device.log.cut().lamport, second.payload().lamport);
    }

    #[test]
    fn settled_transactions_come_out_in_timeline_order() {
        let (mut phone_device, mut laptop_device) = (Device::new(phone()), Device::new(laptop()));
        phone_device.write();
        let theirs = laptop_device.write();
        phone_device.receive(&theirs);
        let reply = phone_device.write();

        let floor = [(phone(), 2), (laptop(), 1)].into_iter().collect();
        let settled = phone_device.log.settle(&floor);
        let stamps: Vec<_> = settled
            .iter()
            .map(|it| it.payload().stamp(it.author()))
            .collect();

        assert_eq!(settled.len(), 3);
        assert!(stamps.is_sorted());
        assert_eq!(stamps.last(), Some(&reply.payload().stamp(phone())));
        assert_eq!(phone_device.log.missing_from(&VersionVector::default()), []);
    }

    #[test]
    fn a_peers_cut_settles_what_was_still_in_the_frontier() {
        let (mut phone_device, mut laptop_device) = (Device::new(phone()), Device::new(laptop()));
        let first = phone_device.write();
        let second = phone_device.write();
        laptop_device.receive(&first);

        // The phone settled both. The laptop only ever saw the first.
        let cut = Cut {
            settled: [(phone(), 2)].into_iter().collect(),
            lamport: second.payload().lamport,
        };
        let absorbed = laptop_device.log.absorb(&cut);

        assert_eq!(absorbed, [first]);
        assert_eq!(laptop_device.log.integrated(), &cut.settled);
        assert_eq!(laptop_device.log.cut(), &cut);
        assert!(laptop_device.write().payload().lamport > cut.lamport);
    }
}
