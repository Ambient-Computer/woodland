//! Checkpoints: proposing them, voting on them, and committing what
//! they cover.
//!
//! A checkpoint's body names, for each clock, the last entry of each
//! author's chain that it covers. Everything it covers and the
//! checkpoint before it didn't is committed, clock by clock, in
//! timeline order, after everything committed earlier.
//! Whatever is left sorts after that, wherever its stamps would have put
//! it: committed work is never reordered by work that arrives late.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::actor::{Identity, Peer};
use crate::chain::Checkpoint;
use crate::db::ClockId;
use crate::encoding;
use crate::store::{StoreError, View};

use super::super::entry::{Frontier, Link, reach};
use super::super::step::Step;
use super::Group;
use super::clock::Clock;
use super::root::Root;

/// What a checkpoint makes final: its body. It names every clock that
/// has anything committed, changed by this checkpoint or not.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Covers {
    pub(super) clocks: BTreeMap<ClockId, Covered>,
}

/// What a checkpoint makes final in one clock.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Covered {
    /// The last entry of each author's chain that it covers. An entry
    /// names the one before it, so this names every entry covered.
    pub(super) frontier: Frontier,
    /// The state every device reaches by committing them.
    pub(super) state: Root,
}

impl Covers {
    /// What's covered of `clock`. Nothing, if it isn't named.
    pub(super) fn frontier(&self, clock: ClockId) -> Frontier {
        let covered = self.clocks.get(&clock);

        covered.map(|it| it.frontier.clone()).unwrap_or_default()
    }
}

/// What `checkpoint` covers. The first checkpoint covers nothing.
pub(super) fn covers_of(checkpoint: &Checkpoint) -> Option<Covers> {
    if checkpoint.height == 0 {
        return Some(Covers::default());
    }

    encoding::decode(&checkpoint.body)
}

impl Group {
    /// Checks every proposed checkpoint this device hasn't vouched for
    /// yet. A vote is a claim to hold everything the checkpoint covers,
    /// and to have reached the state it names by committing it, so this
    /// device votes for nothing it hasn't worked out for itself.
    ///
    /// A witness is the exception. It holds nothing to check against,
    /// so it vouches only for the chain's shape.
    pub(super) async fn vet<V: View>(
        &mut self,
        me: Peer,
        step: &Step<V>,
    ) -> Result<(), StoreError> {
        let witness = self.chain.tip().electorate.is_witness(me);
        let proposed: Vec<Checkpoint> = self.chain.proposed().cloned().collect();
        for checkpoint in proposed {
            let digest = checkpoint.digest();
            // Nothing could follow a checkpoint whose electorate has
            // nobody to vote, or nobody to work out what to vote for.
            if self.sound.contains(&digest) || !checkpoint.electorate.is_viable() {
                continue;
            }

            let sound = if witness {
                self.keeps_going(&checkpoint)
            } else {
                self.checks_out(&checkpoint, step).await?
            };
            if sound {
                self.sound.insert(digest);
            }
        }

        Ok(())
    }

    /// Whether `checkpoint` covers at least what the tip does, in every
    /// clock the tip names.
    fn keeps_going(&self, checkpoint: &Checkpoint) -> bool {
        let before = covers_of(self.chain.tip()).unwrap_or_default();
        let Some(covers) = covers_of(checkpoint) else {
            return false;
        };

        let kept = |(&id, covered): (&ClockId, &Covered)| {
            let frontier = covers.frontier(id);
            let reaches = |(&author, link): (&Peer, &Link)| reach(&frontier, author) >= link.seq;

            covers.clocks.contains_key(&id) && covered.frontier.iter().all(reaches)
        };

        before.clocks.iter().all(kept)
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

        // A clock with anything committed stays named from then on.
        let kept = |(id, clock): (&ClockId, &Clock)| {
            clock.history.committed.is_empty() || covers.clocks.contains_key(id)
        };
        // Vouching for a checkpoint is vouching for every clock in it.
        if self.partial() || self.folded != tip.height || !self.clocks.iter().all(kept) {
            return Ok(false);
        }

        let proven = self.proven();
        let mut trial = step.fork();
        for (&id, covered) in &covers.clocks {
            let unheard = Clock::new(self.id, id);
            let clock = self.clocks.get(&id).unwrap_or(&unheard);
            let writers = tip.electorate.writers(id).except(&proven);
            if !clock.history.supports(&covered.frontier, writers) {
                return Ok(false);
            }

            let settled = clock.settle(&covered.frontier, &mut trial).await?;
            if settled.state != covered.state {
                return Ok(false);
            }
        }

        Ok(true)
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
        let Some(rank) = tip.electorate.members().position(|peer| peer == me) else {
            return Ok(());
        };

        let patience = 2 + u64::try_from(rank).unwrap_or(u64::MAX - 2);
        if tip.electorate.is_reader(me) {
            return Ok(());
        }

        // A witness has no state to commit to, so it starts no attempt
        // of its own. It can carry on with a checkpoint someone else
        // proposed, if that attempt has gone nowhere.
        if tip.electorate.is_witness(me) {
            if self.stalled > patience && self.chain.finish(identity) {
                self.said = true;
                self.stalled = 0;
            }

            return Ok(());
        }

        let free = self.chain.attempt().is_none() || self.stalled > patience;
        if !free || self.folded != tip.height || self.partial() {
            return Ok(());
        }

        // Whoever is proven to have forked goes, if this device has a
        // say in who stays and someone would be left to carry on.
        let electorate = self.wanted.clone();
        let electorate = electorate.unwrap_or_else(|| tip.electorate.clone());
        let proven = self.proven();
        let honest = electorate.without(&proven);
        let purge = tip.electorate.includes(me) && honest.is_viable();
        let electorate = if purge { honest } else { electorate };

        // As much as can be covered: an entry written on top of one
        // that isn't held waits for it.
        let frontiers = self.clocks.iter().map(|(&id, clock)| {
            let writers = tip.electorate.writers(id).except(&proven);

            (id, clock.history.closed(writers, &proven))
        });
        let frontiers: Vec<(ClockId, Frontier)> = frontiers.collect();
        let news = |(id, frontier): &(ClockId, Frontier)| {
            let committed = self.clocks.get(id).map(|clock| &clock.history.committed);

            committed != Some(frontier)
        };
        if !frontiers.iter().any(news) && electorate == tip.electorate {
            return Ok(());
        }

        let mut trial = step.fork();
        let mut covers = Covers::default();
        for (id, frontier) in frontiers {
            let Some(clock) = self.clocks.get(&id) else {
                continue;
            };

            let state = clock.settle(&frontier, &mut trial).await?.state;
            covers.clocks.insert(id, Covered { frontier, state });
        }
        if self
            .chain
            .propose(identity, electorate, encoding::encode(&covers))
        {
            self.said = true;
            self.stalled = 0;
        }

        Ok(())
    }

    /// Commits what the next decided checkpoint covers, once all of it
    /// is held. Returns whether it did.
    pub(super) async fn fold<V: View>(&mut self, step: &mut Step<V>) -> Result<bool, StoreError> {
        let before = self.chain.electorates_since(self.folded).next().cloned();
        let checkpoint = self.chain.at(self.folded + 1).cloned();
        let (Some(before), Some(checkpoint)) = (before, checkpoint) else {
            return Ok(false);
        };

        // Every clock held, and every clock named: a clock that isn't
        // named stays as it is.
        let covers = covers_of(&checkpoint).unwrap_or_default();
        let ids = self.clocks.keys().chain(covers.clocks.keys());
        let ids: BTreeSet<ClockId> = ids.copied().collect();
        let mut plan = Vec::new();
        let mut passed = false;
        for id in ids {
            let covered = covers.clocks.get(&id);
            if !self.wants(id) {
                passed |= covered.is_some_and(|it| !it.frontier.is_empty());
                continue;
            }

            let Some(clock) = self.clocks.get(&id) else {
                if covered.is_some_and(|it| !it.frontier.is_empty()) {
                    return Ok(false);
                }
                continue;
            };

            let frontier = covered.map(|it| it.frontier.clone());
            let frontier = frontier.unwrap_or_else(|| clock.history.committed.clone());
            if clock.history.contradicts(&frontier) {
                tracing::error!(
                    group = ?self.id,
                    clock = ?id,
                    height = checkpoint.height,
                    "holding an entry its author replaced, can't commit"
                );
                return Ok(false);
            }
            if !clock.history.holds(&frontier) {
                return Ok(false);
            }
            plan.push((id, frontier, covered.map(|it| it.state)));
        }

        for (id, frontier, named) in plan {
            let Some(clock) = self.clocks.get_mut(&id) else {
                continue;
            };

            let settled = clock.settle(&frontier, step).await?;
            if named.is_some_and(|named| named != settled.state) {
                tracing::error!(
                    group = ?self.id,
                    clock = ?id,
                    height = checkpoint.height,
                    "a quorum committed a state this device didn't reach"
                );
            }

            clock.commit(&frontier, settled, step);
            clock.cut(checkpoint.electorate.writers(id), checkpoint.height, step);
        }

        self.folded = checkpoint.height;
        self.passed |= passed;
        self.sound.clear();
        self.silence_all(step);

        // A request to reconfigure was made of the electorate as it was.
        // Once that changes, however it changes, the request is spent.
        if checkpoint.electorate != before {
            self.wanted = None;
        }

        Ok(true)
    }
}
