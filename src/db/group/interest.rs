//! Interest: holding only the clocks a device wants.
//!
//! A group can have clocks a device has no use for. One that says
//! which it wants is sent nothing of the rest, keeps nothing of them,
//! and commits checkpoints without them: a checkpoint covers each
//! clock separately, and one clock's state is never needed to work out
//! another's.
//!
//! What it gives up is a say. Voting for a checkpoint is vouching for
//! all of it, so a device with weight wants everything.

use std::collections::BTreeSet;

use crate::actor::Identity;
use crate::db::{Changed, ClockId, keys};
use crate::store::{StoreError, View};

use super::super::step::Step;
use super::Group;
use super::sync::Remote;

/// Why a device can't change which clocks it wants.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// It has a say, and has to hold everything it vouches for.
    Votes,
    /// It would want a clock it has already let checkpoints pass
    /// without. There's no state to start it from.
    Passed,
}

impl Group {
    /// Whether this device holds `clock`, or would if it had any of it.
    pub(crate) fn wants(&self, clock: ClockId) -> bool {
        self.interest.as_ref().is_none_or(|it| it.contains(&clock))
    }

    /// Whether this device holds the state of every clock `remote`
    /// wants, and so can send it a snapshot.
    pub(super) fn serves(&self, remote: &Remote) -> bool {
        match (&self.interest, remote.interest()) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(mine), Some(theirs)) => theirs.is_subset(mine),
        }
    }

    /// The height `remote` has to have committed to be sent entries:
    /// below it, something it wants has been shed here.
    pub(super) fn shed_below(&self, remote: &Remote) -> u64 {
        let wanted = self.clocks.iter().filter(|(id, _)| remote.wants(**id));
        let bases = wanted.map(|(_, clock)| clock.base);

        bases.max().unwrap_or(0).max(self.base)
    }

    /// Whether some clocks are left out.
    pub(super) fn partial(&self) -> bool {
        self.interest.is_some()
    }

    /// Changes which clocks this device wants: only these, or with
    /// `None`, all of them. Whatever it holds of the others is deleted.
    pub(crate) async fn want<V: View>(
        &mut self,
        identity: &Identity,
        clocks: Option<BTreeSet<ClockId>>,
        step: &mut Step<V>,
    ) -> Result<Result<(), Refused>, StoreError> {
        if clocks.is_some() && self.chain.tip().electorate.includes(identity.peer()) {
            return Ok(Err(Refused::Votes));
        }

        // Wanting more is fine while nothing has gone by unheld.
        let wider = match (&self.interest, &clocks) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(old), Some(new)) => !new.is_subset(old),
        };
        if wider && self.passed {
            return Ok(Err(Refused::Passed));
        }

        self.interest = clocks;
        let held = self.clocks.keys().copied();
        let unwanted: Vec<ClockId> = held.filter(|&id| !self.wants(id)).collect();
        for id in unwanted {
            self.let_go(id, step).await?;
        }

        // Peers hear with the next summary, and send less.
        step.changed(Changed::Everything { group: self.id });
        self.staging = None;
        let (start, end) = keys::range(keys::staged(self.id));
        for (key, _) in step.scan(&start, &end).await? {
            step.delete(key);
        }
        self.progress(identity, None, step).await?;
        let peers: Vec<_> = self.remotes.keys().copied().collect();
        for peer in peers {
            self.push(peer, true, false, step).await?;
        }

        Ok(Ok(()))
    }

    /// Deletes everything held of a clock.
    async fn let_go<V: View>(&mut self, id: ClockId, step: &mut Step<V>) -> Result<(), StoreError> {
        let Some(clock) = self.clocks.remove(&id) else {
            return Ok(());
        };

        // Its state is gone, and what the next checkpoint builds on
        // with it.
        self.passed |= !clock.history.committed.is_empty();
        let clock = id.as_bytes();
        let ranges = [
            [&keys::committed(self.id)[..], clock].concat(),
            [&keys::uncommitted(self.id)[..], clock].concat(),
            [&keys::logs(self.id)[..], clock].concat(),
            keys::state(self.id, id),
        ];
        for prefix in ranges {
            let (start, end) = keys::range(prefix);
            for (key, _) in step.scan(&start, &end).await? {
                step.delete(key);
            }
        }

        Ok(())
    }
}
