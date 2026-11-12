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
//! - **Only writers' transactions.** Others wait until their author has
//!   write access.
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
use crate::agreement::{AgreementId, Configuration};
use crate::clock::VersionVector;
use crate::lattice::Lattice;
use crate::model::{ModelId, Operation};
pub(crate) use transaction::Transaction;

/// One log on this device.
///
/// Invariant: `transactions` holds exactly those in `floor..integrated`.
/// Everything at or below `floor` lives on only in model data, either
/// because it was compacted or because it arrived in a snapshot.
#[derive(Debug, Default)]
pub(crate) struct Log {
    clock: Clock,
    /// The highest sequence number this device has authored, including
    /// transactions still waiting for write access.
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
    /// Below this, transactions are no longer kept individually.
    pub(crate) floor: VersionVector,
}

impl Log {
    /// Rebuilds from storage. Stored transactions beyond the clock are this
    /// device's own, still waiting for write access.
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

    pub(crate) fn floor(&self) -> &VersionVector {
        &self.clock.floor
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
    /// only what's integrated, and whose author may write. Returns them in
    /// integration order, for the models.
    ///
    /// Write access is "ever a member of the learned configuration". Removal
    /// can't revoke past writes: replicas learn about a removal at different
    /// times, so cutting off a removed author's writes would make replicas
    /// disagree about which of them count.
    pub(crate) fn integrate_ready(
        &mut self,
        writers: &Configuration,
    ) -> Vec<Verified<Transaction>> {
        let mut integrated = Vec::new();
        while let Some(key) = self.next_ready(writers) {
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
    /// what's in the log: anything below the floor must go as a snapshot.
    pub(crate) fn missing_from(&self, theirs: &VersionVector) -> Vec<Signed<Transaction>> {
        self.transactions
            .iter()
            .filter(|&(&(author, seq), _)| !theirs.contains(author, seq))
            .map(|(_, transaction)| transaction.signed().clone())
            .collect()
    }

    /// Raises the floor and forgets the transactions below it. Returns,
    /// per author, the sequence number forgotten through.
    pub(crate) fn compact(&mut self, floor: &VersionVector) -> Vec<(Peer, u64)> {
        let mut forgotten = Vec::new();
        for (author, through) in floor.iter() {
            if through > self.clock.floor.get(author) {
                self.clock.floor.advance(author, through);
                forgotten.push((author, through));
            }
        }

        self.transactions
            .retain(|&(author, seq), _| !self.clock.floor.contains(author, seq));

        forgotten
    }

    /// Takes on a peer's snapshot: everything at or below its floor is now
    /// reflected in the model data, so this device has it too.
    /// Returns what was forgotten, as [`Log::compact`] does.
    pub(crate) fn absorb(&mut self, floor: &VersionVector, lamport: u64) -> Vec<(Peer, u64)> {
        for (author, through) in floor.iter() {
            self.clock.integrated.advance(author, through);
        }

        self.clock.lamport = self.clock.lamport.max(lamport);
        let integrated = &self.clock.integrated;
        self.waiting
            .retain(|&(author, seq), _| !integrated.contains(author, seq));

        self.compact(floor)
    }

    fn next_ready(&self, writers: &Configuration) -> Option<(Peer, u64)> {
        let mut waiting = self.waiting.iter();
        let (&key, _) = waiting.find(|&(&(author, seq), transaction)| {
            seq == self.clock.integrated.next(author)
                && transaction.payload().after.leq(&self.clock.integrated)
                && writers.was_ever_member(author)
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
            let integrated = self.log.integrate_ready(&genesis().configuration());

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
}
