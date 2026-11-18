//! How much of a group a device keeps: shedding history, getting it
//! back, wanting less, and letting go.

use std::collections::BTreeSet;

use ed25519_dalek::VerifyingKey;

use crate::actor::Peer;
use crate::store::Store;

use super::{Changed, ClockId, Database, Error, GroupId, Outputs, group, keys};

impl<S: Store> Database<S> {
    /// About how many bytes the store holds that it has freed and
    /// not given back to the device. Deleting, [compacting], and
    /// letting blobs go free space for the store to use again, and
    /// needn't shrink it: [`reclaim`] does.
    ///
    /// A store that can't tell says none. The browser's is one.
    ///
    /// [compacting]: Self::compact
    /// [`reclaim`]: Self::reclaim
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn unreturned(&self) -> Result<u64, Error> {
        Ok(self.store.unreturned().await?)
    }

    /// Gives back to the device what the store has freed. It may
    /// take as long as rewriting everything the store holds, so
    /// it's for when the device is idle and [`unreturned`] says
    /// there's enough to be worth it.
    ///
    /// [`unreturned`]: Self::unreturned
    ///
    /// # Errors
    ///
    /// If the store fails. Nothing is lost, and the database carries
    /// on as it was.
    pub async fn reclaim(&mut self) -> Result<(), Error> {
        if self.broken {
            return Err(Error::Broken);
        }

        Ok(self.store.reclaim().await?)
    }

    /// Sheds the history of `group` that's committed, keeping committed
    /// state and whatever isn't committed yet. Nothing reads differently
    /// afterwards. A peer that needs what was shed is sent the state
    /// instead.
    ///
    /// A device that never calls this is an archive: it can hand any
    /// peer the group's whole history.
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails.
    pub async fn compact(&mut self, group: GroupId) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let shed = held.shed(&self.identity, None, &mut step).await;
        self.guard(shed)?;

        self.finish(step).await
    }

    /// Sheds the committed history of one clock of `group`, as
    /// [`compact`] does for all of them. The other clocks keep theirs,
    /// and this device can still hand it to a peer that wants only
    /// those.
    ///
    /// [`compact`]: Self::compact
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails.
    pub async fn compact_clock(
        &mut self,
        group: GroupId,
        clock: ClockId,
    ) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let shed = held.shed(&self.identity, Some(clock), &mut step).await;
        self.guard(shed)?;

        self.finish(step).await
    }

    /// Starts getting back the history of `clock` that this device
    /// shed, from any peer that kept it: an archive. Each entry is
    /// checked against what this device kept, so it's as good as
    /// history that never left. It arrives over time; [`is_archive`]
    /// says when it's all here. Compacting again stops it.
    ///
    /// Reads don't change. What changes is that this device can hand
    /// the history on.
    ///
    /// [`is_archive`]: Self::is_archive
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails.
    pub async fn restore(&mut self, group: GroupId, clock: ClockId) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let restoring = held.restore(&self.identity, clock, &mut step).await;
        self.guard(restoring)?;

        self.finish(step).await
    }

    /// Has this device catch up on `group` by being sent what's
    /// committed, in place of every transaction that led to it. It's
    /// the same state either way, and less to send and to check for a
    /// device that's a long way behind. What isn't committed yet still
    /// arrives as transactions.
    ///
    /// It holds until the device has caught up once. The history
    /// skipped isn't kept here: see [`restore`].
    ///
    /// [`restore`]: Self::restore
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails.
    pub async fn skip_ahead(&mut self, group: GroupId) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let skipping = held.skip(&self.identity, &mut step).await;
        self.guard(skipping)?;

        self.finish(step).await
    }

    /// Whether this device holds every entry of `clock` that was ever
    /// committed, and every checkpoint: it shed none, or has them all
    /// back. Such a device can bring a peer up to date entry by entry,
    /// however far behind it is.
    #[must_use]
    pub fn is_archive(&self, group: GroupId, clock: ClockId) -> bool {
        self.groups.get(&group).is_some_and(|it| it.is_whole(clock))
    }

    /// Holds only these clocks of `group`, or with `None`, all of
    /// them, which is how a group starts out. Nothing of the other
    /// clocks is kept, asked for, or sent here, and reads of them come
    /// back empty. Writing to one is refused.
    ///
    /// Wanting less is always possible. Wanting more is possible until
    /// a checkpoint has been committed here without a clock it covers:
    /// so choose straight after [joining].
    ///
    /// [joining]: Self::join
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, has a say in it, or has
    /// passed over a clock it's now asking for, or the store fails.
    pub async fn want(
        &mut self,
        group: GroupId,
        clocks: Option<BTreeSet<ClockId>>,
    ) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let wanted = held.want(&self.identity, clocks, &mut step).await;
        match self.guard(wanted)? {
            Ok(()) => self.finish(step).await,
            Err(group::Refused::Votes) => Err(Error::Votes),
            Err(group::Refused::Passed) => Err(Error::PassedOver),
        }
    }

    /// Asks `peer` for only these clocks of `group`, or with `None`,
    /// for every clock this device holds, which is how a connection
    /// starts out. It lasts until the peer is disconnected, and can be
    /// changed at any time.
    ///
    /// It's for a connection that shouldn't carry everything: a
    /// metered one, or one to a peer trusted with only so much. What's
    /// held back still arrives over other connections, and nothing is
    /// committed here until it has. It narrows what's sent as it's
    /// written, not what [want] has this device hold, and not the
    /// committed state a device that's far behind is sent in place of
    /// history, which comes whole.
    ///
    /// [want]: Self::want
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails. A
    /// peer that isn't connected is asked nothing.
    pub async fn ask(
        &mut self,
        peer: VerifyingKey,
        group: GroupId,
        clocks: Option<BTreeSet<ClockId>>,
    ) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let asked = held.ask(Peer::from(peer), clocks, &mut step).await;
        self.guard(asked)?;

        self.finish(step).await
    }

    /// Stops holding `group`, and deletes everything kept for it.
    ///
    /// Only a device with no role in the group can. One that could
    /// still write or vote has signed things it would forget, and
    /// would sign them again differently if it [joined] again: it has
    /// to be removed from the group first, by the group.
    ///
    /// [joined]: Self::join
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group` or is still a member of it,
    /// or the store fails.
    pub async fn forget(&mut self, group: GroupId) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get(&group).ok_or(Error::UnknownGroup)?;
        if held.has(self.identity.peer()) {
            return Err(Error::StillAMember);
        }

        let (start, end) = keys::range(keys::group(group));
        let kept = step.scan(&start, &end).await;
        for (key, _) in self.guard(kept)? {
            step.delete(key);
        }
        step.delete(keys::founding(group));
        step.changed(Changed::Everything { group });
        self.groups.remove(&group);

        self.finish(step).await
    }
}
