//! Transactions in hand: the few whose contents are in memory right now.
//!
//! The log keeps an outline of each transaction and leaves the rest in
//! storage. Three things need the whole transaction back:
//!
//! - **sending** it to a peer that lacks it (see `sync`);
//! - **settling** it, which folds its values into the settled state;
//! - **showing** it, when it turns out to be live some time after it was
//!   integrated (see `verdicts`), and its versions have to be written.
//!
//! Each asks for a page at a time, at most [`PAGE`] transactions, and
//! picks up the answer in a later batch. So however long the frontier
//! gets, what's in memory and what goes in one message stay bounded.
//!
//! A transaction is also in hand during the batch that integrates it,
//! which is when most of them are sent and shown.
//!
//! Between batches, only the ones waiting on a verdict are kept, and
//! no more than a page of them. A verdict waits on one read of storage,
//! so they're about to be shown, and reading them back as well would
//! make that two reads in a row.

use std::collections::{BTreeMap, BTreeSet};

use crate::actor::{Peer, Verified};
use crate::agreement::AgreementId;
use crate::log::Transaction;
use crate::node::Outbox;
pub(super) use crate::storage::PAGE;
use crate::storage::keys;

#[derive(Debug)]
pub(super) struct Bodies {
    agreement: AgreementId,
    /// What's in hand this batch. `None` for a transaction storage was
    /// asked for and doesn't have.
    held: BTreeMap<(Peer, u64), Option<Verified<Transaction>>>,
    /// What was asked for this batch, so each is read once.
    asked: BTreeSet<(Peer, u64)>,
}

impl Bodies {
    pub(super) fn new(agreement: AgreementId) -> Self {
        Self {
            agreement,
            held: BTreeMap::new(),
            asked: BTreeSet::new(),
        }
    }

    /// A transaction was just integrated.
    pub(super) fn hold(&mut self, transaction: Verified<Transaction>) {
        let key = (transaction.author(), transaction.payload().seq);
        self.held.insert(key, Some(transaction));
    }

    /// Storage answered: the transaction, or that it holds none.
    pub(super) fn read(&mut self, key: (Peer, u64), stored: Option<Verified<Transaction>>) {
        let held = self.held.entry(key).or_default();
        if held.is_none() {
            *held = stored;
        }
    }

    pub(super) fn get(&self, key: (Peer, u64)) -> Option<&Verified<Transaction>> {
        self.held.get(&key)?.as_ref()
    }

    /// Whether it's known what storage holds for `key`. Everything asked
    /// for in one batch is answered in one batch.
    pub(super) fn answered(&self, key: (Peer, u64)) -> bool {
        self.held.contains_key(&key)
    }

    /// Asks storage for a transaction. The answer is in hand for one
    /// batch, whenever it comes.
    pub(super) fn ask(&mut self, key: (Peer, u64), outbox: &mut Outbox) {
        if self.asked.insert(key) {
            outbox.get(keys::log(self.agreement, key.0, key.1));
        }
    }

    /// The batch is over. Lets go of everything but the first page of
    /// transactions that are `awaited`.
    pub(super) fn clear(&mut self, awaited: impl Fn((Peer, u64)) -> bool) {
        let mut kept = 0;
        self.held.retain(|key, held| {
            let keep = held.is_some() && kept < PAGE && awaited(*key);
            kept += usize::from(keep);
            keep
        });
        self.asked.clear();
    }
}
