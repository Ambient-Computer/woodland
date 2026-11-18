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
    /// The group is sealed. With no secret held, the device carries
    /// the group and reads nothing.
    Sealed {
        /// Every secret this device knows the group to have had, by
        /// the height of the checkpoint that brought it in, and the
        /// secret itself where it's held.
        secrets: BTreeMap<u64, Option<Secret>>,
        /// What the group's state is named under. It's the same for
        /// the life of the group.
        blind: Option<Secret>,
        /// Whether the device gave up the secrets it held, to carry
        /// the group and not read it. It takes none until the group
        /// names it to read again.
        given_up: bool,
    },
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
    /// A sealed group's keys, before any are held.
    pub(crate) fn sealed() -> Self {
        Self::Sealed {
            secrets: BTreeMap::from([(0, None)]),
            blind: None,
            given_up: false,
        }
    }

    pub(crate) fn is_sealed(&self) -> bool {
        matches!(self, Self::Sealed { .. })
    }

    /// What the group's state is named under, if it's held.
    pub(crate) fn blind(&self) -> Option<&Secret> {
        match self {
            Self::Open => None,
            Self::Sealed { blind, .. } => blind.as_ref(),
        }
    }

    /// Lets go of every secret, keeping which were brought in when.
    /// What's left is what a device that never read the group has.
    pub(crate) fn give_up(&mut self) {
        if let Self::Sealed {
            secrets,
            blind,
            given_up,
        } = self
        {
            for secret in secrets.values_mut() {
                *secret = None;
            }
            *blind = None;
            *given_up = true;
        }
    }

    /// Whether the secrets were given up, and none is to be taken.
    pub(crate) fn is_given_up(&self) -> bool {
        matches!(self, Self::Sealed { given_up: true, .. })
    }

    /// The group named this device to read again.
    pub(crate) fn take_up(&mut self) {
        if let Self::Sealed { given_up, .. } = self {
            *given_up = false;
        }
    }

    /// Keeps what the group's state is named under.
    pub(crate) fn hold_blind(&mut self, secret: Secret) {
        if let Self::Sealed { blind, .. } = self {
            *blind = Some(secret);
        }
    }

    /// Names a state by `sum`, which stands for everything in it. In a
    /// sealed group the name says nothing to anyone who can't read the
    /// group, even if they can guess what the state is.
    pub(crate) fn name(&self, sum: &[u8]) -> [u8; 32] {
        match self.blind() {
            Some(blind) if self.is_sealed() => blind.name(sum),
            _ => blake3::derive_key("woodland/state/root", sum),
        }
    }

    /// The secret brought in at `epoch`, if it's held.
    pub(crate) fn secret(&self, epoch: u64) -> Option<&Secret> {
        match self {
            Self::Open => None,
            Self::Sealed { secrets, .. } => secrets.get(&epoch)?.as_ref(),
        }
    }

    /// The secret in force under the checkpoint at `height`: the one
    /// brought in latest by then, if this device holds it.
    fn in_force(&self, height: u64) -> Option<&Secret> {
        match self {
            Self::Open => None,
            Self::Sealed { secrets, .. } => secrets.range(..=height).next_back()?.1.as_ref(),
        }
    }

    /// Notes that a secret was brought in at `epoch`. Returns whether
    /// that's news.
    pub(crate) fn know(&mut self, epoch: u64) -> bool {
        match self {
            Self::Open => false,
            Self::Sealed { secrets, .. } => {
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
            Self::Sealed { secrets, .. } => secrets.insert(epoch, Some(secret)).flatten().is_none(),
        }
    }

    /// Makes `plain` ready to travel: as it is in an open group, and
    /// encrypted under the secret in force at `height` in a sealed
    /// one. `None` if that secret isn't held.
    pub(crate) fn seal(&self, about: &[u8], height: u64, plain: Vec<u8>) -> Option<Vec<u8>> {
        match self {
            Self::Open => Some(plain),
            Self::Sealed { .. } => Some(self.in_force(height)?.seal(about, &plain)),
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
