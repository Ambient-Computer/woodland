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
