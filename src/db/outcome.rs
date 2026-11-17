use ed25519_dalek::VerifyingKey;

use super::{ClockId, GroupId};

/// The name of a transaction: whose it is, and where it sits in what
/// they've written to a clock of a group. [`Database::write`] gives one for what
/// it wrote.
///
/// [`Database::write`]: crate::Database::write
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TransactionId {
    pub group: GroupId,
    pub clock: ClockId,
    pub author: VerifyingKey,
    pub seq: u64,
}

/// What's become of a transaction this device wrote.
///
/// The first two are provisional, and can follow one another any number
/// of times. The rest are final.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// Everything it requires holds, so far. Optimistic reads show it.
    Live,
    /// Something it requires doesn't hold, so far. Nothing shows it.
    Pruned,
    /// A checkpoint covers it, and it took effect.
    Committed,
    /// A checkpoint covers it, and it didn't take effect.
    Rejected,
    /// It will never be committed: its author was removed before a
    /// checkpoint covered it.
    Discarded,
    /// A checkpoint covers it, and this device caught up from a
    /// snapshot, so it never worked out whether it took effect.
    Covered,
}

impl Outcome {
    const ALL: [Self; 6] = [
        Self::Live,
        Self::Pruned,
        Self::Committed,
        Self::Rejected,
        Self::Discarded,
        Self::Covered,
    ];

    /// How it's stored.
    pub(crate) fn to_byte(self) -> u8 {
        match self {
            Self::Live => 0,
            Self::Pruned => 1,
            Self::Committed => 2,
            Self::Rejected => 3,
            Self::Discarded => 4,
            Self::Covered => 5,
        }
    }

    pub(crate) fn from_bytes(stored: &[u8]) -> Option<Self> {
        let [byte] = stored else {
            return None;
        };

        Self::ALL.into_iter().find(|it| it.to_byte() == *byte)
    }
}
