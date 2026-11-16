//! Fork proofs: two things one contributor signed that it couldn't
//! both have signed honestly.
//!
//! A contributor appends to its own chain, so it signs one entry at
//! each place in it. Two headers at one place, signed by one key, need
//! no witness and no context to convict: anyone can check them, and
//! they stay true forever. The one exception is in the chain of
//! checkpoints, not here: a contributor that was removed and let back
//! in writes again from where it was cut, over entries that no longer
//! count.

use serde::{Deserialize, Serialize};

use crate::actor::{Peer, Signed, verify_all};
use crate::db::{ClockId, GroupId};

use super::entry::Header;

/// Two headers by one author at one place in one of its chains.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Proof {
    first: Signed<Header>,
    second: Signed<Header>,
}

/// What a proof shows, once its signatures hold.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fork {
    pub(crate) author: Peer,
    pub(crate) clock: ClockId,
    /// The heights of the checkpoints the two entries were written
    /// under, lower first.
    pub(crate) under: (u64, u64),
}

impl Proof {
    pub(crate) fn new(first: Signed<Header>, second: Signed<Header>) -> Self {
        Self { first, second }
    }

    /// Checks the proof on its own: both headers are signed by the
    /// same key, for the same place in `group`, and they differ.
    pub(crate) fn check(&self, group: GroupId) -> Option<Fork> {
        let signed = vec![self.first.clone(), self.second.clone()];
        let [first, second] = <[_; 2]>::try_from(verify_all(signed)).ok()?;
        let (a, b) = (first.payload(), second.payload());
        let same = first.author() == second.author()
            && a.group == group
            && b.group == group
            && a.clock == b.clock
            && a.seq == b.seq;
        let (lower, higher) = (a.under.height, b.under.height);

        (same && a != b).then_some(Fork {
            author: first.author(),
            clock: a.clock,
            under: (lower.min(higher), lower.max(higher)),
        })
    }
}
