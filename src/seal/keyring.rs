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
    /// The group is sealed. Each secret this device holds, by the
    /// height of the checkpoint that brought it in. With none, the
    /// device carries the group and reads nothing.
    Sealed(BTreeMap<u64, Secret>),
}

impl Keyring {
    pub(crate) fn is_sealed(&self) -> bool {
        matches!(self, Self::Sealed(_))
    }

    /// The secret brought in at `height`, if it's held.
    pub(crate) fn secret(&self, height: u64) -> Option<&Secret> {
        match self {
            Self::Open => None,
            Self::Sealed(secrets) => secrets.get(&height),
        }
    }

    /// The secret in force under the checkpoint at `height`: the latest
    /// one brought in by then, of those held.
    fn in_force(&self, height: u64) -> Option<&Secret> {
        match self {
            Self::Open => None,
            Self::Sealed(secrets) => secrets.range(..=height).next_back().map(|(_, it)| it),
        }
    }

    /// Keeps a secret. Returns whether it's new.
    pub(crate) fn hold(&mut self, height: u64, secret: Secret) -> bool {
        match self {
            Self::Open => false,
            Self::Sealed(secrets) => secrets.insert(height, secret).is_none(),
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

    /// Undoes [`seal`]. `None` if this device can't read it.
    ///
    /// [`seal`]: Self::seal
    pub(crate) fn open(&self, about: &[u8], height: u64, sealed: &[u8]) -> Option<Vec<u8>> {
        match self {
            Self::Open => Some(sealed.to_vec()),
            Self::Sealed(_) => self.in_force(height)?.open(about, sealed),
        }
    }
}
