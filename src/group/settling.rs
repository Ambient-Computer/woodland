//! Settling: using agreement to find the part of the timeline that can no
//! longer change.
//!
//! # Why the timeline changes at all
//!
//! Devices write offline, so a transaction can arrive long after it was
//! written, with a stamp that puts it in the middle of everyone else's
//! timeline. Models derive their state from the timeline's order, so until
//! that can't happen anymore, the state at any point in it is provisional.
//!
//! # The floor
//!
//! Every contributor regularly proposes its own version vector into the
//! agreement. A learned [`Decision`] then holds a progress report from each
//! contributor, and its **floor** is their greatest lower bound: the
//! transactions every current contributor has integrated.
//!
//! What agreement adds over gossiping those reports is **who counts**. The
//! contributor set is part of the same decision, so the floor is always
//! taken over a membership everyone eventually agrees on, and only once
//! every one of them has acknowledged it (see [`Decision::floor`]).
//!
//! # From the floor to the settled prefix
//!
//! The floor alone isn't final. A transaction above it can be concurrent
//! with one below it, and sort before it. So what settles is the longest
//! **prefix of the timeline** that lies within the floor: everything up to
//! the first transaction that some contributor hasn't integrated yet.
//!
//! That prefix is final once nothing can still arrive that would sort
//! into it. Stamps respect causality, so that means: everything this
//! device hasn't integrated was written on top of the whole floor. Going
//! through who could still write something:
//!
//! - **A contributor.** What it writes follows what it last reported,
//!   which includes the floor. What it wrote before that, it reported, and
//!   this device waits until it holds everything the contributors have
//!   reported before it settles anything.
//! - **A newcomer.** It isn't admitted until it has caught up to what the
//!   contributors before it hold (see `admission`).
//! - **A removed contributor.** Its writes only count if a contributor
//!   vouches for them, which makes them part of what was reported (see
//!   `removal`).
//!
//! # Every device settles the same history
//!
//! The timeline is a function of the set of transactions, so every device
//! that settles is settling a prefix of the same eventual timeline. Two
//! prefixes of one list are comparable: one device may be ahead of
//! another, but they never disagree, and a device that's behind can take
//! the state of one that's ahead as it is. That's what a snapshot is.
//!
//! Lattice agreement gives comparable decisions, not a sequence of them,
//! and devices skip decisions freely. That's enough here, because the
//! floor only decides how *far* to settle. The *order* never depended on
//! it.
//!
//! # What settling buys
//!
//! - **Committed reads.** Settled state is the same on every device that
//!   has it, and only ever moves forward.
//! - **Final verdicts.** Whether a transaction's requirements held can't
//!   change once nothing can land before it (see `verdicts`).
//! - **Bounded storage.** A settled transaction leaves the log, and its
//!   writes fold into one settled value per key (see `persistence`). A
//!   removal that settles leaves nothing: no write that would have lost to
//!   it can arrive anymore, so there's no tombstone to keep.
//! - **Bounded hydration.** Only what hasn't settled loads at startup.

use crate::agreement::Decision;
use crate::lattice::Lattice;
use crate::log::{Log, Outline};

/// The start of the timeline that `learned` proves is final, up to
/// `limit` transactions, in timeline order. Settling them takes their
/// contents, which are in storage, so it happens a page at a time.
///
/// Nothing settles until this device holds everything the contributors
/// have reported. An observer that's behind can learn a floor long before
/// it has the transactions to settle it. And nothing settles past a
/// transaction that isn't `ready`: one whose verdict is still out (see
/// `verdicts`).
pub(super) fn settleable(
    log: &Log,
    learned: &Decision,
    ready: impl Fn(&Outline) -> bool,
    limit: usize,
) -> Vec<Outline> {
    let contributors = learned.configuration.members();
    let reported = learned.progress.ceiling(&contributors);
    if !reported.leq(log.integrated()) {
        return Vec::new();
    }

    log.settleable(&learned.floor(), ready, limit)
}
