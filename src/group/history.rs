use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::actor::{Peer, Signed, Verified};
use crate::agreement::{AgreementId, Configuration};
use crate::clock::VersionVector;
use crate::model::{ModelId, Operation, Transaction};

/// The agreement's clock and transaction log on this device.
///
/// Invariant: the log holds exactly the transactions in `floor..integrated`.
/// Everything at or below `floor` is folded into model state and no longer
/// kept individually, either because it was compacted or because it
/// arrived in a snapshot.
#[derive(Debug, Default)]
pub(crate) struct History {
    clock: Clock,
    /// The highest sequence number this device has authored, including
    /// transactions still waiting for write access.
    authored: u64,
    log: BTreeMap<(Peer, u64), Verified<Transaction>>,
    /// Verified transactions that can't be integrated yet: there's a gap
    /// before them in their author's history, or their author doesn't have
    /// write access yet. They're re-checked whenever either might change.
    waiting: BTreeMap<(Peer, u64), Verified<Transaction>>,
}

/// The parts of a [`History`] that are persisted as one record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Clock {
    /// Per author, how far this device has integrated.
    pub(crate) integrated: VersionVector,
    /// The greatest Lamport counter seen. New transactions go above it.
    pub(crate) lamport: u64,
    /// Below this, transactions are no longer kept individually.
    pub(crate) floor: VersionVector,
}

impl History {
    /// Rebuilds from storage. Stored transactions beyond the clock are this
    /// device's own, still waiting for write access.
    pub(crate) fn restore(clock: Clock, stored: Vec<Verified<Transaction>>, me: Peer) -> Self {
        let mut history = Self {
            authored: clock.integrated.get(me),
            clock,
            ..Self::default()
        };

        for transaction in stored {
            let author = transaction.author();
            let seq = transaction.payload().seq;
            if author == me {
                history.authored = history.authored.max(seq);
            }

            let destination = if history.clock.integrated.contains(author, seq) {
                &mut history.log
            } else {
                &mut history.waiting
            };
            destination.insert((author, seq), transaction);
        }

        history
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

    /// The next transaction this device will author. It takes the next
    /// sequence number and a Lamport counter above everything seen, so it
    /// wins over every write this device knows about.
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
            lamport: self.clock.lamport,
            operations,
        }
    }

    /// Accepts a verified transaction. It waits until it can be integrated.
    pub(crate) fn offer(&mut self, transaction: Verified<Transaction>) {
        let key = (transaction.author(), transaction.payload().seq);
        if !self.clock.integrated.contains(key.0, key.1) {
            self.waiting.entry(key).or_insert(transaction);
        }
    }

    /// Integrates every waiting transaction that's now in order and whose
    /// author may write. Returns them in integration order, for the models.
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
                self.log.insert(key, transaction.clone());
                integrated.push(transaction);
            }
        }

        integrated
    }

    /// Transactions a peer that has integrated `theirs` is missing. Only
    /// what's in the log: anything below the floor must go as a snapshot.
    pub(crate) fn missing_from(&self, theirs: &VersionVector) -> Vec<Signed<Transaction>> {
        self.log
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

        self.log
            .retain(|&(author, seq), _| !self.clock.floor.contains(author, seq));

        forgotten
    }

    /// Takes on a peer's snapshot: everything at or below its floor is now
    /// reflected in the merged model state, so this device has it too.
    /// Returns what was forgotten, as [`History::compact`] does.
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
        self.waiting.keys().copied().find(|&(author, seq)| {
            seq == self.clock.integrated.next(author) && writers.was_ever_member(author)
        })
    }
}
