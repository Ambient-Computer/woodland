use std::fmt;

use serde::{Deserialize, Serialize};

/// Names a group: the hash of the checkpoint it was founded with.
///
/// Peers use it to say which group a message is about, and storage uses
/// it to scope every key the group owns.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GroupId([u8; 32]);

impl GroupId {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for GroupId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "GroupId(")?;
        for byte in &self.0[..4] {
            write!(formatter, "{byte:02x}")?;
        }

        write!(formatter, "…)")
    }
}

/// Names a clock: one causal history within a group, and the models it
/// owns.
///
/// A clock exists once something is written to one of its models. A
/// group's first clock is named after the group, so a [`GroupId`] can
/// be used wherever a clock is asked for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ClockId([u8; 32]);

impl ClockId {
    /// Another clock in `group`. The application supplies the nonce:
    /// random bytes, or a name every device derives alike.
    #[must_use]
    pub fn new(group: GroupId, nonce: [u8; 32]) -> Self {
        Self(crate::encoding::hash(
            "woodland/clock",
            &(group.as_bytes(), nonce),
        ))
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<GroupId> for ClockId {
    fn from(group: GroupId) -> Self {
        Self(group.0)
    }
}

impl fmt::Debug for ClockId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ClockId(")?;
        for byte in &self.0[..4] {
            write!(formatter, "{byte:02x}")?;
        }

        write!(formatter, "…)")
    }
}
