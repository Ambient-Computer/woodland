//! Founding a group that carries on from another.

use crate::chain::{Chain, Checkpoint, Electorate};
use crate::store::Store;

use super::step::Step;
use super::{Database, Error, GroupId, Outputs};

impl<S: Store> Database<S> {
    /// Starts a group that carries on from `old`, which this device
    /// holds: one that begins with everything `old` has committed, and
    /// with this device as its only contributor. It's for a group that
    /// has lost for good the devices it takes to commit anything, and
    /// it's people who decide that. Nothing can tell gone from away.
    ///
    /// The new group is another group. Its models keep the IDs they
    /// had, so what was read from `old` is read from it the same way.
    /// What wasn't committed in `old` isn't carried over: write it
    /// again. `old` is left as it is, and anyone still there can found
    /// a successor of their own, which will be a different group.
    ///
    /// The state is brought over a part at a time: [resume] until
    /// there's nothing unfinished. Other devices [join] as they would
    /// any group.
    ///
    /// [resume]: Self::resume
    /// [join]: Self::join
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `old`, holds only some of it, is
    /// partway through catching up with it, or can't read it; if `old`
    /// is sealed; or the store fails.
    pub async fn succeed(
        &mut self,
        old: GroupId,
        seed: [u8; 32],
    ) -> Result<(GroupId, Outputs), Error> {
        let held = self.groups.get(&old).ok_or(Error::UnknownGroup)?;
        let body = held.bequest().ok_or(Error::Unreadable)?;
        let me = self.identity.peer();
        let mut founding = Checkpoint::founding(seed, Electorate::new([(me, 1)]));
        founding.body = body;
        let (id, outputs) = self.hold(Chain::new(founding), None).await?;

        let mut step = self.begin()?;
        if let Some(group) = self.groups.get_mut(&id) {
            group.inherit(&mut step);
        }
        let begun = self.finish(step).await?;

        Ok((
            id,
            Outputs {
                sends: outputs.sends.into_iter().chain(begun.sends).collect(),
                changed: outputs.changed.into_iter().chain(begun.changed).collect(),
                ..begun
            },
        ))
    }

    /// The group that `group` carries on from, if it was founded to
    /// [succeed] one.
    ///
    /// [succeed]: Self::succeed
    #[must_use]
    pub fn succeeds(&self, group: GroupId) -> Option<GroupId> {
        Some(self.groups.get(&group)?.succeeds()?.group)
    }

    /// Brings over some more of the state of each group this device
    /// founded to carry on from another.
    pub(super) async fn inherit(&mut self, step: &mut Step<S::View>) -> Result<(), Error> {
        let heirs = self
            .groups
            .iter()
            .filter(|(_, group)| group.is_inheriting());
        let heirs: Vec<GroupId> = heirs.map(|(&id, _)| id).collect();
        for id in heirs {
            let Some(mut heir) = self.groups.remove(&id) else {
                continue;
            };
            let old = heir.succeeds().and_then(|it| self.groups.get(&it.group));
            let inherited = match old {
                Some(old) => heir.inherit_some(old, step).await,
                None => Ok(()),
            };
            self.groups.insert(id, heir);
            self.guard(inherited)?;
        }

        Ok(())
    }
}
