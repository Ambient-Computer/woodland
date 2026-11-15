//! Checkpoints: proposing them, voting on them, and committing what
//! they cover.
//!
//! A checkpoint's body names the last entry of each author's chain that
//! it covers. Everything it covers and the checkpoint before it didn't
//! is committed in timeline order, after everything committed earlier.
//! Whatever is left sorts after that, wherever its stamps would have put
//! it: committed work is never reordered by work that arrives late.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::actor::{Identity, Peer};
use crate::chain::Checkpoint;
use crate::clock::Stamp;
use crate::db::keys;
use crate::encoding;
use crate::model::{ModelId, Operation};
use crate::store::{StoreError, View};

use super::super::entry::Frontier;
use super::super::step::Step;
use super::Group;
use super::judge::{self, Written};
use super::root::{Accumulator, Root};

/// What a checkpoint makes final: its body.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Covers {
    /// The last entry of each author's chain that it covers. An entry
    /// names the one before it, so this names every entry covered.
    pub(super) frontier: Frontier,
    /// The state every device reaches by committing them.
    pub(super) state: Root,
}

/// What committing a checkpoint's entries comes to.
struct Settled {
    state: Root,
    dropped: BTreeSet<ModelId>,
    /// The covered entries, in timeline order, and whether each happens.
    verdicts: Vec<(Stamp, bool)>,
}

/// What `checkpoint` covers. The first checkpoint covers nothing.
pub(super) fn covers_of(checkpoint: &Checkpoint) -> Option<Covers> {
    if checkpoint.height == 0 {
        return Some(Covers {
            frontier: Frontier::new(),
            state: Accumulator::default().root(),
        });
    }

    encoding::decode(&checkpoint.body)
}

impl Group {
    /// Checks every proposed checkpoint this device hasn't vouched for
    /// yet. A vote is a claim to hold everything the checkpoint covers,
    /// and to have reached the state it names by committing it, so this
    /// device votes for nothing it hasn't worked out for itself.
    pub(super) async fn vet<V: View>(&mut self, step: &Step<V>) -> Result<(), StoreError> {
        let proposed: Vec<Checkpoint> = self.chain.proposed().cloned().collect();
        for checkpoint in proposed {
            let digest = checkpoint.digest();
            if !self.sound.contains(&digest) && self.checks_out(&checkpoint, step).await? {
                self.sound.insert(digest);
            }
        }

        Ok(())
    }

    async fn checks_out<V: View>(
        &self,
        checkpoint: &Checkpoint,
        step: &Step<V>,
    ) -> Result<bool, StoreError> {
        let tip = self.chain.tip();
        let Some(covers) = covers_of(checkpoint) else {
            return Ok(false);
        };
        if self.folded != tip.height || !self.history.supports(&covers.frontier, &tip.electorate) {
            return Ok(false);
        }

        let settled = self.settle(&covers.frontier, &mut step.fork()).await?;

        Ok(settled.state == covers.state)
    }

    /// Moves the chain along as far as what's held allows.
    pub(super) fn advance(&mut self, identity: &Identity) -> bool {
        let sound = &self.sound;

        self.chain
            .advance(identity, |checkpoint| sound.contains(&checkpoint.digest()))
    }

    /// Starts an attempt at the next checkpoint, if there's something to
    /// commit and nobody else is at it. Electors wait different lengths
    /// of time before giving up on someone else's attempt, so they don't
    /// all outbid it at once.
    pub(super) async fn propose<V: View>(
        &mut self,
        identity: &Identity,
        step: &Step<V>,
    ) -> Result<(), StoreError> {
        let tip = self.chain.tip();
        let me = identity.peer();
        let Some(rank) = tip.electorate.peers().position(|peer| peer == me) else {
            return Ok(());
        };

        let patience = 2 + u64::try_from(rank).unwrap_or(u64::MAX - 2);
        let free = self.chain.attempt().is_none() || self.stalled > patience;
        let pending = !self.history.uncommitted.is_empty() || self.wanted.is_some();
        let frontier = self.history.heads.clone();
        let sound = self.folded == tip.height && self.history.supports(&frontier, &tip.electorate);
        if !(free && pending && sound) {
            return Ok(());
        }

        let state = self.settle(&frontier, &mut step.fork()).await?.state;
        let covers = Covers { frontier, state };
        let electorate = self.wanted.clone();
        let electorate = electorate.unwrap_or_else(|| tip.electorate.clone());
        if self
            .chain
            .propose(identity, electorate, encoding::encode(&covers))
        {
            self.said = true;
            self.stalled = 0;
        }

        Ok(())
    }

    /// Commits, in `step`, every uncommitted entry `frontier` covers:
    /// each is judged against committed state alone, which by then
    /// includes every entry before it. Nothing in memory changes, so a
    /// step that's thrown away leaves no trace.
    async fn settle<V: View>(
        &self,
        frontier: &Frontier,
        step: &mut Step<V>,
    ) -> Result<Settled, StoreError> {
        let stored = step.get(&keys::state(self.id)).await?;
        let stored = stored.and_then(|bytes| Accumulator::from_bytes(&bytes));
        let mut state = stored.unwrap_or_default();
        let mut dropped = self.dropped.clone();
        let mut verdicts = Vec::new();
        for entry in self.history.covered(frontier) {
            let happens = judge::weigh(self.id, entry, &Written::on(&dropped), step).await?;
            if happens {
                let operations = self.operations(entry, step).await?;
                judge::commit(self.id, &operations, &mut state, step).await?;
                let drops = operations.iter().filter(|it| it.drops());
                dropped.extend(drops.map(|it: &Operation| it.model));
            }
            verdicts.push((entry.stamp(), happens));
        }

        step.put(keys::state(self.id), state.to_bytes());

        Ok(Settled {
            state: state.root(),
            dropped,
            verdicts,
        })
    }

    /// Commits what the next decided checkpoint covers, once all of it
    /// is held. Returns whether it did.
    pub(super) async fn fold<V: View>(&mut self, step: &mut Step<V>) -> Result<bool, StoreError> {
        let before = self.chain.electorates_since(self.folded).next().cloned();
        let checkpoint = self.chain.at(self.folded + 1).cloned();
        let (Some(before), Some(checkpoint)) = (before, checkpoint) else {
            return Ok(false);
        };

        let covers = covers_of(&checkpoint);
        let named = covers.as_ref().map(|covers| covers.state);
        let frontier = covers.map(|covers| covers.frontier);
        let frontier = frontier.unwrap_or_else(|| self.history.committed.clone());
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

        let settled = self.settle(&frontier, step).await?;
        if named != Some(settled.state) {
            tracing::error!(
                group = ?self.id,
                height = checkpoint.height,
                "a quorum committed a state this device didn't reach"
            );
        }

        self.dropped = settled.dropped;
        for (stamp, happened) in settled.verdicts {
            let Some(entry) = self.history.forget(stamp) else {
                continue;
            };

            if happened {
                judge::notify(self.id, &entry, true, step);
            }
            judge::hide(self.id, &entry, step);
            judge::notify(self.id, &entry, false, step);
            self.live.remove(&stamp);
        }

        self.history.commit(&frontier);
        self.folded = checkpoint.height;
        self.sound.clear();
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
            self.void(author, step);
        }
    }

    /// Drops everything `author` wrote that isn't committed.
    pub(super) fn void<V: View>(&mut self, author: Peer, step: &mut Step<V>) {
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
