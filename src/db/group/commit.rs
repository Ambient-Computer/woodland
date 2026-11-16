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
use super::sealing::Sealing;
use super::succession::Succeeds;

/// How many entries a step commits before leaving the rest to the
/// next. Small under test, so that every test meets a checkpoint
/// that takes several.
pub(crate) const STRIDE: usize = if cfg!(test) { 3 } else { 256 };

/// What a checkpoint makes final: its body. It names every clock that
/// has anything committed, changed by this checkpoint or not.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Covers {
    pub(super) clocks: BTreeMap<ClockId, Covered>,
    /// What it says about the group's secret, if the group is sealed.
    pub(super) sealing: Option<Sealing>,
    /// In a group's first checkpoint: the group it carries on from.
    pub(super) succeeds: Option<Succeeds>,
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

impl Covered {
    /// Whether this commits nothing beyond `before`, what the
    /// checkpoint before it did.
    fn stays(&self, before: Option<&Self>) -> bool {
        before.is_some_and(|it| it.frontier == self.frontier && it.state == self.state)
    }
}

impl Covers {
    /// What's covered of `clock`. Nothing, if it isn't named.
    pub(super) fn frontier(&self, clock: ClockId) -> Frontier {
        let covered = self.clocks.get(&clock);

        covered.map(|it| it.frontier.clone()).unwrap_or_default()
    }
}

/// What `checkpoint` covers. The first checkpoint covers nothing, and
/// says something only in a sealed group.
pub(super) fn covers_of(checkpoint: &Checkpoint) -> Option<Covers> {
    if checkpoint.height == 0 && checkpoint.body.is_empty() {
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
        identity: &Identity,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let me = identity.peer();
        let witness = self.chain.tip().electorate.is_witness(me);
        let proposed: Vec<Checkpoint> = self.chain.proposed().cloned().collect();
        self.fore.vetting = false;
        for checkpoint in proposed {
            let digest = checkpoint.digest();
            // Nothing could follow a checkpoint whose electorate has
            // nobody to vote, or nobody to work out what to vote for.
            let judged = self.sound.contains(&digest) || self.fore.refuted.contains(&digest);
            if judged || !checkpoint.electorate.is_viable() {
                continue;
            }

            let found = if witness {
                Some(self.keeps_going(&checkpoint))
            } else {
                self.checks_out(&checkpoint, step).await?
            };
            // One at a time: starting on another would be starting
            // this one again.
            let Some(sound) = found else {
                self.fore.vetting = true;
                break;
            };

            if sound && self.sealed_soundly(identity, &checkpoint) {
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

    /// Whether this device, committing what `checkpoint` covers, comes
    /// to the state it names. `None` while that's being worked out.
    async fn checks_out<V: View>(
        &mut self,
        checkpoint: &Checkpoint,
        step: &mut Step<V>,
    ) -> Result<Option<bool>, StoreError> {
        let tip = self.chain.tip();
        let Some(covers) = covers_of(checkpoint) else {
            return Ok(Some(false));
        };

        // A clock with anything committed stays named from then on.
        let kept = |(id, clock): (&ClockId, &Clock)| {
            clock.history.committed.is_empty() || covers.clocks.contains_key(id)
        };
        // Vouching for a checkpoint is vouching for every clock in it,
        // and there's no vouching for what can't be read.
        if self.partial()
            || self.is_unfounded()
            || self.wanting().is_some()
            || self.folded != tip.height
            || !self.clocks.iter().all(kept)
        {
            return Ok(Some(false));
        }

        let proven = self.proven();
        let before = covers_of(tip).unwrap_or_default();
        let mut frontiers = Vec::new();
        let mut named = Vec::new();
        for (&id, covered) in &covers.clocks {
            let unheard = Clock::new(self.id, id);
            let clock = self.clocks.get(&id).unwrap_or(&unheard);

            // There's no vouching for what can't be read. What can be
            // vouched for is that nothing more of it is committed.
            if clock.is_blind() {
                if !covered.stays(before.clocks.get(&id)) {
                    return Ok(Some(false));
                }
                continue;
            }
            let writers = tip.electorate.writers(id).except(&proven);
            let stood = |peer: Peer, height: u64| self.standing(peer, id, height);
            if !clock.history.supports(&covered.frontier, writers, stood) {
                return Ok(Some(false));
            }

            frontiers.push((id, covered.frontier.clone()));
            named.push(covered.state);
        }

        let Some(reached) = self.ahead(&frontiers, step).await? else {
            return Ok(None);
        };
        // Nothing this device comes to hold changes what it worked
        // out, so there's no asking again.
        if reached != named {
            self.fore.refuted.insert(checkpoint.digest());
        }

        Ok(Some(reached == named))
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
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        // What it set out to propose, if it's still working out what
        // that comes to. Anything that stops it now, stops that.
        let begun = self.fore.proposing.take();
        let tip = self.chain.tip();
        let me = identity.peer();
        let Some(rank) = tip.electorate.members().position(|peer| peer == me) else {
            return Ok(());
        };

        // Rivals that keep outranking each other decide nothing, so
        // each waits longer every time it tries for the same place.
        let height = tip.height;
        let tries = if self.tried.0 == height {
            self.tried.1
        } else {
            0
        };
        let patience = 2 + u64::try_from(rank).unwrap_or(u64::MAX - 2);
        let patience = patience << tries.min(6);
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
        let held_up = self.folded != tip.height || self.partial() || self.wanting().is_some();
        let held_up = held_up || self.is_unfounded();
        // Checking someone else's attempt comes before making another.
        if !free || held_up || self.fore.vetting {
            return Ok(());
        }

        // Whoever is proven to have forked goes, if this device has a
        // say in who stays and someone would be left to carry on.
        let electorate = self.asked_for();
        let electorate = electorate.unwrap_or_else(|| tip.electorate.clone());
        let proven = self.proven();
        let honest = electorate.without(&proven);
        let purge = tip.electorate.includes(me) && honest.is_viable();
        let electorate = if purge { honest } else { electorate };

        // As much as can be covered: an entry written on top of one
        // that isn't held waits for it.
        let frontiers = self.clocks.iter().map(|(&id, clock)| {
            // A clock this device can't read stays where it is.
            if clock.is_blind() {
                return (id, clock.history.committed.clone());
            }

            let writers = tip.electorate.writers(id).except(&proven);

            let stood = |peer: Peer, height: u64| self.standing(peer, id, height);

            (id, clock.history.closed(writers, &proven, stood))
        });
        let frontiers: Vec<(ClockId, Frontier)> = frontiers.collect();
        // More may have come in since it set out. What it set out with
        // is still as good, and is nearly worked out.
        let within = |(begun, now): (&(ClockId, Frontier), &(ClockId, Frontier))| {
            let reaches = |(&author, link): (&Peer, &Link)| reach(&now.1, author) >= link.seq;

            begun.0 == now.0 && begun.1.iter().all(reaches)
        };
        let frontiers = match begun {
            Some(begun)
                if begun.len() == frontiers.len() && begun.iter().zip(&frontiers).all(within) =>
            {
                begun
            }
            _ => frontiers,
        };
        let news = |(id, frontier): &(ClockId, Frontier)| {
            let committed = self.clocks.get(id).map(|clock| &clock.history.committed);

            committed != Some(frontier)
        };
        if !frontiers.iter().any(news) && electorate == tip.electorate {
            return Ok(());
        }

        let sealing = self.seal_next(identity, &electorate);
        if self.is_sealed() && sealing.is_none() {
            return Ok(());
        }

        let before = covers_of(tip).unwrap_or_default();
        let blind = |id: &ClockId| self.clocks.get(id).is_none_or(Clock::is_blind);
        let read = frontiers.iter().filter(|(id, _)| !blind(id));
        let read: Vec<(ClockId, Frontier)> = read.cloned().collect();
        let kept = frontiers
            .iter()
            .filter(|(id, _)| self.clocks.get(id).is_some_and(Clock::is_blind));
        let kept = kept.filter_map(|(id, _)| Some((*id, before.clocks.get(id)?.clone())));
        let mut covers = Covers {
            clocks: kept.collect(),
            sealing,
            succeeds: None,
        };
        let Some(states) = self.ahead(&read, step).await? else {
            self.fore.proposing = Some(frontiers);
            return Ok(());
        };

        for ((id, frontier), state) in read.into_iter().zip(states) {
            covers.clocks.insert(id, Covered { frontier, state });
        }
        if self
            .chain
            .propose(identity, electorate, encoding::encode(&covers))
        {
            self.said = true;
            self.stalled = 0;
            self.tried = (height, tries + 1);
        }

        Ok(())
    }

    /// Commits what the next decided checkpoint covers, once all of it
    /// is held. Returns whether it did.
    pub(super) async fn fold<V: View>(&mut self, step: &mut Step<V>) -> Result<bool, StoreError> {
        // Committed state is in pieces until a snapshot is in place.
        if self.installing.is_some() {
            return Ok(false);
        }

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

        // A checkpoint can cover any amount of work. A step commits
        // so much of it, in timeline order, and the next carries on.
        let mut budget = STRIDE;
        let mut whole = true;
        for (id, frontier, named) in &plan {
            let Some(clock) = self.clocks.get_mut(id) else {
                continue;
            };

            if !clock.work(frontier, &mut budget, step).await? {
                whole = false;
                break;
            }

            let reached = clock.root(step).await?;
            if !clock.is_lost() && named.is_some_and(|named| named != reached) {
                tracing::error!(
                    group = ?self.id,
                    clock = ?id,
                    height = checkpoint.height,
                    "a quorum committed a state this device didn't reach"
                );
            }
        }
        self.folding = (!whole).then_some(checkpoint.height);
        if !whole {
            return Ok(false);
        }

        for (id, ..) in plan {
            let writers = checkpoint.electorate.writers(id);
            self.clock(id).cut(writers, checkpoint.height, step);
        }

        self.folded = checkpoint.height;
        self.passed |= passed;
        self.sound.clear();
        self.silence_all(step);

        // A request to reconfigure was made of the electorate as it was.
        // Once that changes, however it changes, the request is spent.
        if checkpoint.electorate != before {
            self.wanted = None;
            self.requests.clear();
        }

        Ok(true)
    }
}
