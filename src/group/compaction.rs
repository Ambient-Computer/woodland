//! Compaction: using agreement to find history that nobody needs anymore.
//!
//! # The floor
//!
//! Every contributor regularly proposes its own version vector into the
//! agreement. A learned [`Decision`] then holds a progress report from each
//! contributor, and its **floor** is their greatest lower bound: the
//! transactions every current contributor has integrated. No contributor
//! will ever ask for those individually again, so the log can drop them.
//! Their effects stay in model data, and a device that falls behind the
//! floor (a returning observer, a new member) catches up from a snapshot.
//!
//! # What agreement adds
//!
//! The floor is just a function of a decision, and a gossiped progress map
//! would give one too. What agreement adds is **who counts**. The
//! contributor set is part of the same decision as the reports, so the floor
//! is always taken over a membership everyone eventually agrees on. Floors
//! taken from learned decisions are comparable too, so every device's
//! compaction windows nest the same way.
//!
//! # Where it stops being monotone
//!
//! Adding a member who hasn't reported yet pulls the floor of the next
//! decision back to zero (see [`Progress::floor`]). The floor here is a high
//! watermark: it never goes back down, because the transactions are
//! already gone. That's safe for the log. The newcomer gets a snapshot
//! instead of the transactions, and merging a snapshot is just a union of
//! versions.
//!
//! # What this floor is *not* enough for
//!
//! Dropping **tombstones** needs more. A tombstone can only go once no
//! write concurrent with it can ever arrive. The floor says every current
//! member has seen the tombstone, which rules out concurrent writes from
//! them. But:
//!
//! - **A new member** admitted concurrently may have been bootstrapped from
//!   a state below the floor, so its first writes can be concurrent with
//!   what everyone else already compacted.
//! - **A removed member** may still hold writes nobody else has seen. The
//!   floor stops counting it, but its old writes are still valid (removal
//!   can't revoke them deterministically, see `History::integrate_ready`).
//!
//! Both need a deterministic cut-off point. Lattice agreement offers
//! comparability, not a "first decision that included this change", so
//! neither falls out of it directly. That's the main place where RLA and
//! causal-stability-based garbage collection fight. Tombstones are kept.
//!
//! [`Progress::floor`]: crate::clock::Progress::floor

use super::history::History;
use crate::actor::Peer;
use crate::agreement::Decision;

/// Raises the floor to what `learned` proves every contributor has, and
/// returns what was forgotten (per author, through which sequence number).
///
/// The agreed floor is clamped to what this device has integrated itself.
/// An observer that's behind can learn a floor it hasn't reached yet, and
/// it must never claim to hold transactions it doesn't have.
pub(super) fn compact(history: &mut History, learned: &Decision) -> Vec<(Peer, u64)> {
    let agreed = learned.progress.floor(&learned.configuration.members());
    let reachable = agreed.meet(history.integrated());

    history.compact(&reachable)
}
