//! Groups only their readers can read.

use crate::chain::{Chain, Checkpoint, Electorate, Role};
use crate::seal::SealingKey;
use crate::store::Store;

use super::group::{self, Group};
use super::{Database, Error, GroupId, Outputs, keys};

impl<S: Store> Database<S> {
    /// The key this device is sent a sealed group's secrets under.
    /// Whoever gives this device a role in one needs it: see
    /// [`Role::sealed_to`] and [`Role::reader`].
    #[must_use]
    pub fn sealing_key(&self) -> SealingKey {
        SealingKey::of(&self.identity)
    }

    /// Replaces the key this device is sent secrets under with one
    /// made from `fresh`: random bytes, used for nothing else. It's
    /// for a key that may have got out, and it's the only way back
    /// from that short of a new identity.
    ///
    /// Nothing changes in any group until it's [reconfigured] to name
    /// the new key for this device, which also replaces the group's
    /// secret. Until then this device is sent secrets under the old
    /// key, and it goes on opening them: every key it has had is kept.
    ///
    /// [reconfigured]: Self::reconfigure
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn replace_sealing_key(
        &mut self,
        fresh: [u8; 32],
    ) -> Result<(SealingKey, Outputs), Error> {
        let mut step = self.begin()?;
        self.identity.renew(fresh);
        step.save(keys::sealing(), &self.identity.renewals().to_vec());
        let outputs = self.finish(step).await?;

        Ok((self.sealing_key(), outputs))
    }

    /// Starts a sealed group: one whose contents only its contributors
    /// and readers can read, whoever else carries it. A group is open
    /// or sealed for good.
    ///
    /// Everyone given a role that reads has to be given it with their
    /// [sealing key]. Witnesses and devices with no role hold a sealed
    /// group and pass it on without being able to read it.
    ///
    /// [sealing key]: Self::sealing_key
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn found_sealed(&mut self, seed: [u8; 32]) -> Result<(GroupId, Outputs), Error> {
        let me = self.identity.peer();
        let role = Role::contributor(1).sealed_to(self.sealing_key());
        let (secret, body) = group::found_sealed(&self.identity, seed);
        let mut founding = Checkpoint::founding(seed, Electorate::of([(me, role)]));
        founding.body = body;

        self.hold(Chain::new(founding), Some(secret)).await
    }

    /// Whether this device can read `group`: it's open, or this device
    /// holds its secret. A device that can't still holds the group,
    /// and its reads come back empty.
    #[must_use]
    pub fn reads(&self, group: GroupId) -> bool {
        self.groups.get(&group).is_some_and(Group::reads)
    }

    /// Asks a sealed group to replace its secret with the next
    /// checkpoint, for the same readers. It's for a secret that may
    /// have got out: whoever holds the old one reads everything
    /// written under it, and nothing written after.
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`; if it has no say in it, or
    /// lacks the secret, which goes for every device if the group is
    /// open; or the store fails.
    pub async fn replace_secret(&mut self, group: GroupId) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let asked = held.renew(&self.identity, &mut step).await;
        if !self.guard(asked)? {
            return Err(Error::Unreadable);
        }

        self.finish(step).await
    }
}
