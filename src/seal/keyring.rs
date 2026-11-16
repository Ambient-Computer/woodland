//! What a device can read of a group.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::secret::Secret;

/// The secrets a device holds for a group, if the group has any.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Keyring {
    /// The group is open. Nothing in it is encrypted.
    #[default]
    Open,
    /// The group is sealed. Every secret this device knows the group
    /// to have had, by the height of the checkpoint that brought it
    /// in, and the secret itself where it's held. With none held, the
    /// device carries the group and reads nothing.
    Sealed(BTreeMap<u64, Option<Secret>>),
}

/// What came of opening something.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Opened {
    Plain(Vec<u8>),
    /// This device lacks the secret it was sealed under, or doesn't
    /// know which that is.
    Unreadable,
    /// It isn't something sealed under the secret it should have been.
    /// Everyone who holds that secret finds the same.
    Garbled,
}

impl Keyring {
    pub(crate) fn is_sealed(&self) -> bool {
        matches!(self, Self::Sealed(_))
    }

    /// The secret brought in at `epoch`, if it's held.
    pub(crate) fn secret(&self, epoch: u64) -> Option<&Secret> {
        match self {
            Self::Open => None,
            Self::Sealed(secrets) => secrets.get(&epoch)?.as_ref(),
        }
    }

    /// The secret in force under the checkpoint at `height`: the one
    /// brought in latest by then, if this device holds it.
    fn in_force(&self, height: u64) -> Option<&Secret> {
        match self {
            Self::Open => None,
            Self::Sealed(secrets) => secrets.range(..=height).next_back()?.1.as_ref(),
        }
    }

    /// Notes that a secret was brought in at `epoch`. Returns whether
    /// that's news.
    pub(crate) fn know(&mut self, epoch: u64) -> bool {
        match self {
            Self::Open => false,
            Self::Sealed(secrets) => {
                let news = !secrets.contains_key(&epoch);
                secrets.entry(epoch).or_insert(None);

                news
            }
        }
    }

    /// Keeps a secret. Returns whether it wasn't held before.
    pub(crate) fn hold(&mut self, epoch: u64, secret: Secret) -> bool {
        match self {
            Self::Open => false,
            Self::Sealed(secrets) => secrets.insert(epoch, Some(secret)).flatten().is_none(),
        }
    }

    /// Makes `plain` ready to travel: as it is in an open group, and
    /// encrypted under the secret in force at `height` in a sealed
    /// one. `None` if that secret isn't held.
    pub(crate) fn seal(&self, about: &[u8], height: u64, plain: Vec<u8>) -> Option<Vec<u8>> {
        match self {
            Self::Open => Some(plain),
            Self::Sealed(_) => Some(self.in_force(height)?.seal(about, &plain)),
        }
    }

    /// Undoes [`seal`].
    ///
    /// [`seal`]: Self::seal
    pub(crate) fn open(&self, about: &[u8], height: u64, sealed: &[u8]) -> Opened {
        if !self.is_sealed() {
            return Opened::Plain(sealed.to_vec());
        }
        let Some(secret) = self.in_force(height) else {
            return Opened::Unreadable;
        };

        let plain = secret.open(about, sealed);

        plain.map_or(Opened::Garbled, Opened::Plain)
    }
}
