use ed25519_dalek::VerifyingKey;

use super::ClockId;

/// The name of a transaction: whose it is, and where it sits in what
/// they've written to the clock. [`Database::write`] gives one for what
/// it wrote.
///
/// [`Database::write`]: crate::Database::write
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TransactionId {
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
