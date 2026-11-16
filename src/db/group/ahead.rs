//! Working out what a checkpoint would come to, before it's decided.
//!
//! A vote for a checkpoint vouches for the state it names, so a device
//! with a say commits what the checkpoint covers for itself first, and
//! compares. It can't do that to its committed state: the checkpoint
//! might never be decided. So it does it to one side. Everything the
//! work would write is kept under other keys, where no read looks, and
//! the work reads those before anything else.
//!
//! Like any commit it's done a part at a time. Where it's got to is
//! kept in memory alone: a device that stops starts it again, and
//! clears away what the last attempt left.

use std::collections::BTreeSet;

use crate::chain::Digest;
use crate::db::keys;
use crate::store::{StoreError, View};

use super::super::entry::Frontier;
use super::super::step::Step;
use super::clock::Clock;
use super::commit::STRIDE;
use super::compact::SWEEP;
use super::root::Root;
use super::{ClockId, Group};

#[derive(Debug)]
pub(super) struct Ahead {
    /// Each clock, what's committed in it, and what's to be.
    goal: Goal,
    /// The clocks as they'll be. As many as have `roots` are done.
    clocks: Vec<Clock>,
    roots: Vec<Root>,
    /// Whether anyone asked after it during this step.
    asked: bool,
}

type Goal = Vec<(ClockId, Frontier, Frontier)>;

/// Everything a group keeps about working things out ahead.
#[derive(Debug, Default)]
pub(super) struct Foresight {
    pub(super) work: Option<Ahead>,
    /// Whether anything worked out ahead might still be stored.
    pub(super) littered: bool,
    /// Proposed checkpoints that name a state this device worked out
    /// differently, and won't vote for.
    pub(super) refuted: BTreeSet<Digest>,
    /// Whether a proposed checkpoint is waiting to be worked out.
    pub(super) vetting: bool,
    /// What this device means to propose, once it's worked out what
    /// it comes to.
    pub(super) proposing: Option<Vec<(ClockId, Frontier)>>,
    /// Whether it's time to try for a checkpoint.
    pub(super) due: bool,
}

impl Ahead {
    fn is_done(&self) -> bool {
        self.roots.len() == self.clocks.len()
    }
}

impl Group {
    /// The name of the state each clock comes to once what `frontiers`
    /// cover is committed, in the same order. `None` until that's been
    /// worked out, which takes as many steps as it takes: ask again.
    pub(super) async fn ahead<V: View>(
        &mut self,
        frontiers: &[(ClockId, Frontier)],
        step: &mut Step<V>,
    ) -> Result<Option<Vec<Root>>, StoreError> {
        let from = |id: &ClockId| self.clocks.get(id).map(|it| it.history.committed.clone());
        let goal = frontiers.iter();
        let goal = goal.map(|(id, frontier)| (*id, from(id).unwrap_or_default(), frontier.clone()));
        let goal: Goal = goal.collect();
        if self
            .fore
            .work
            .as_ref()
            .is_none_or(|ahead| ahead.goal != goal)
        {
            // What the last one left would be taken for this one's.
            self.fore.work = None;
            self.tidy(step).await?;
            if self.fore.littered {
                return Ok(None);
            }

            let clock = |(id, ..): &(ClockId, Frontier, Frontier)| {
                let held = self.clocks.get(id).cloned();

                held.unwrap_or_else(|| Clock::new(self.id, *id))
            };
            self.fore.work = Some(Ahead {
                clocks: goal.iter().map(clock).collect(),
                goal,
                roots: Vec::new(),
                asked: false,
            });
        }

        let Some(mut ahead) = self.fore.work.take() else {
            return Ok(None);
        };
        ahead.asked = true;
        if !ahead.is_done() {
            self.fore.littered = true;
            step.divert(Some((keys::group(self.id), keys::ahead(self.id))));
            let worked = ahead.work(step).await;
            step.divert(None);
            worked?;
        }

        let roots = ahead.is_done().then(|| ahead.roots.clone());
        self.fore.work = Some(ahead);

        Ok(roots)
    }

    /// Whether working something out ahead is still going on.
    pub(super) fn is_ahead(&self) -> bool {
        self.fore.littered
            || self
                .fore
                .work
                .as_ref()
                .is_some_and(|ahead| !ahead.is_done())
    }

    /// Gives up on what nobody asked after in this step, and clears
    /// away some of what's left of anything given up on.
    pub(super) async fn look_back<V: View>(
        &mut self,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let asked = |ahead: &mut Ahead| std::mem::take(&mut ahead.asked);
        if !self.fore.work.as_mut().is_some_and(asked) {
            self.fore.work = None;
            self.tidy(step).await?;
        }

        Ok(())
    }

    async fn tidy<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        if !self.fore.littered {
            return Ok(());
        }

        let (start, end) = keys::range(keys::ahead(self.id));
        let page = step.scan_page(&start, &end, SWEEP).await?;
        self.fore.littered = page.len() == SWEEP;
        for (key, _) in page {
            step.delete(key);
        }

        Ok(())
    }
}

#[cfg(test)]
impl Group {
    /// What every clock comes to with everything held committed, or,
    /// if not `all`, with nothing more committed than is.
    pub(crate) async fn foresee<V: View>(
        &mut self,
        all: bool,
        step: &mut Step<V>,
    ) -> Result<Option<Vec<Root>>, StoreError> {
        let frontier = |clock: &Clock| {
            let history = &clock.history;

            if all {
                history.heads.clone()
            } else {
                history.committed.clone()
            }
        };
        let frontiers = self.clocks.iter().map(|(&id, clock)| (id, frontier(clock)));
        let frontiers: Vec<_> = frontiers.collect();

        self.ahead(&frontiers, step).await
    }
}

impl Ahead {
    /// Commits some more of it, in a step that keeps it to one side.
    async fn work<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        let mut budget = STRIDE;
        while let Some(clock) = self.clocks.get_mut(self.roots.len()) {
            let (.., frontier) = &self.goal[self.roots.len()];
            if !clock.work(frontier, &mut budget, step).await? {
                break;
            }
            self.roots.push(clock.root(step).await?);
        }

        Ok(())
    }
}
