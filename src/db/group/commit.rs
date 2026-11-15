//! Checkpoints: proposing them, voting on them, and committing what
//! they cover.
//!
//! A checkpoint's body names the last entry of each author's chain that
//! it covers. Everything it covers and the checkpoint before it didn't
//! is committed in timeline order, after everything committed earlier.
//! Whatever is left sorts after that, wherever its stamps would have put
//! it: committed work is never reordered by work that arrives late.

use std::collections::BTreeSet;

use crate::actor::{Identity, Peer};
use crate::chain::Checkpoint;
use crate::clock::Stamp;
use crate::encoding;
use crate::model::Operation;
use crate::storage::keys;
use crate::store::{StoreError, View};

use super::super::entry::{Frontier, Outline};
use super::super::step::Step;
use super::Group;
use super::judge::{self, Written};

/// What `checkpoint` covers. The first checkpoint covers nothing.
pub(super) fn frontier_of(checkpoint: &Checkpoint) -> Option<Frontier> {
    if checkpoint.height == 0 {
        return Some(Frontier::new());
    }

    encoding::decode(&checkpoint.body)
}

impl Group {
    /// Moves the chain along as far as what's held allows. A vote is a
    /// claim to hold everything the checkpoint covers, so this device
    /// votes for nothing it hasn't checked against its own copy.
    pub(super) fn advance(&mut self, identity: &Identity) -> bool {
        let tip = self.chain.tip();
        let caught_up = self.folded == tip.height;
        let writers = tip.electorate.clone();
        let history = &self.history;
        let valid = |checkpoint: &Checkpoint| {
            let covers = frontier_of(checkpoint);

            caught_up && covers.is_some_and(|frontier| history.supports(&frontier, &writers))
        };

        self.chain.advance(identity, valid)
    }

    /// Starts an attempt at the next checkpoint, if there's something to
    /// commit and nobody else is at it. Electors wait different lengths
    /// of time before giving up on someone else's attempt, so they don't
    /// all outbid it at once.
    pub(super) fn propose(&mut self, identity: &Identity) {
        let tip = self.chain.tip();
        let me = identity.peer();
        let Some(rank) = tip.electorate.peers().position(|peer| peer == me) else {
            return;
        };

        let patience = 2 + u64::try_from(rank).unwrap_or(u64::MAX - 2);
        let free = self.chain.attempt().is_none() || self.stalled > patience;
        let pending = !self.history.uncommitted.is_empty() || self.wanted.is_some();
        let frontier = self.history.heads.clone();
        let sound = self.folded == tip.height && self.history.supports(&frontier, &tip.electorate);
        if !(free && pending && sound) {
            return;
        }

        let electorate = self.wanted.clone();
        let electorate = electorate.unwrap_or_else(|| tip.electorate.clone());
        if self
            .chain
            .propose(identity, electorate, encoding::encode(&frontier))
        {
            self.said = true;
            self.stalled = 0;
        }
    }

    /// Commits what the next decided checkpoint covers, once all of it
    /// is held. Returns whether it did.
    pub(super) async fn fold<V: View>(&mut self, step: &mut Step<V>) -> Result<bool, StoreError> {
        let next =
            usize::try_from(self.folded).map_or(usize::MAX, |folded| folded.saturating_add(1));
        let mut pair = self.chain.checkpoints().skip(next - 1).take(2).cloned();
        let (Some(before), Some(checkpoint)) = (pair.next(), pair.next()) else {
            return Ok(false);
        };
        drop(pair);
        let before = before.electorate;

        let covers = frontier_of(&checkpoint);
        let frontier = covers.unwrap_or_else(|| self.history.committed.clone());
        if self.history.contradicts(&frontier) {
            tracing::error!(
                group = ?self.id,
                height = checkpoint.height,
                "holding an entry its author replaced, can't commit"
            );
            return Ok(false);
        }
        if !self.history.holds(&frontier) {
            return Ok(false);
        }

        let covered = self.history.covered(&frontier);
        let covered: Vec<Stamp> = covered.map(Outline::stamp).collect();
        for stamp in covered {
            let Some(entry) = self.history.forget(stamp) else {
                continue;
            };

            // Each is judged against committed state alone, which by now
            // includes every entry before it.
            let written = Written::on(&self.dropped);
            if judge::weigh(self.id, &entry, &written, step).await? {
                let operations = self.operations(&entry, step).await?;
                judge::commit(self.id, &operations, step).await?;
                let drops = operations.iter().filter(|it| it.drops());
                self.dropped.extend(drops.map(|it: &Operation| it.model));
                judge::notify(self.id, &entry, true, step);
            }

            judge::hide(self.id, &entry, step);
            judge::notify(self.id, &entry, false, step);
            self.live.remove(&stamp);
        }

        self.history.commit(&frontier);
        self.folded = checkpoint.height;
        self.cut(&checkpoint, step);

        // A request to reconfigure was made of the electorate as it was.
        // Once that changes, however it changes, the request is spent.
        if checkpoint.electorate != before {
            self.wanted = None;
        }
        self.stale = true;

        Ok(true)
    }

    /// Lets go of whatever was written, past this checkpoint, by anyone
    /// it leaves without a say. That's the whole of removal: a removed
    /// contributor's history ends at the checkpoint that removed it.
    fn cut<V: View>(&mut self, checkpoint: &Checkpoint, step: &mut Step<V>) {
        let authors = self.history.uncommitted.values().map(|entry| entry.author);
        let removed = authors.filter(|&author| !checkpoint.electorate.includes(author));
        let removed: BTreeSet<Peer> = removed.collect();
        for author in removed {
            let entries = self.history.cut(author);
            tracing::info!(
                group = ?self.id,
                ?author,
                entries = entries.len(),
                "dropped what a removed contributor wrote after its last checkpoint"
            );
            for entry in entries {
                step.delete(keys::log(self.id, author, entry.link.seq));
                judge::hide(self.id, &entry, step);
                judge::notify(self.id, &entry, false, step);
                self.live.remove(&entry.stamp());
            }
        }
    }
}
