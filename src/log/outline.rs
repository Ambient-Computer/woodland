use super::Transaction;
use crate::actor::{Peer, Verified};
use crate::clock::Stamp;
use crate::model::{Declaration, Operation};

/// What the log remembers of a transaction it has integrated: where it
/// sits in the timeline, and what its operations declare. No values. The
/// transaction itself is in storage, and is read back when it's needed:
/// to send it, to settle it, or to write what it leaves behind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Outline {
    pub(crate) author: Peer,
    pub(crate) seq: u64,
    pub(crate) lamport: u64,
    /// What each operation declares, in order.
    pub(crate) declared: Vec<Declaration>,
}

impl Outline {
    pub(crate) fn of(transaction: &Verified<Transaction>) -> Self {
        let payload = transaction.payload();
        let operations = payload.operations.iter();

        Self {
            author: transaction.author(),
            seq: payload.seq,
            lamport: payload.lamport,
            declared: operations.flat_map(Operation::declares).collect(),
        }
    }

    /// Names the transaction: its author, and its place in their history.
    pub(crate) fn key(&self) -> (Peer, u64) {
        (self.author, self.seq)
    }

    pub(crate) fn stamp(&self) -> Stamp {
        Stamp {
            lamport: self.lamport,
            author: self.author,
        }
    }
}
