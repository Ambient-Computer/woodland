//! Proofs: two things one member signed that it couldn't both have
//! signed honestly.
//!
//! There are two kinds. A fork is two entries at one place in a chain
//! of the log. A conflict is two statements about one checkpoint that
//! [agreement](crate::chain::Evidence) rules out.
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
use crate::chain::{Conflict, Evidence};
use crate::db::{ClockId, GroupId};

use super::entry::Header;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Proof {
    /// Two headers by one author at one place in one of its chains.
    Fork(Box<[Signed<Header>; 2]>),
    /// Two statements about one checkpoint.
    Conflict(Box<Evidence>),
}

/// What a proof shows, once its signatures hold.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Offence {
    Fork(Fork),
    Conflict(Conflict),
}

impl Offence {
    pub(crate) fn author(&self) -> Peer {
        match self {
            Self::Fork(fork) => fork.author,
            Self::Conflict(conflict) => conflict.author,
        }
    }
}

/// Two entries where there should be one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fork {
    pub(crate) author: Peer,
    pub(crate) clock: ClockId,
    /// The heights of the checkpoints the two entries were written
    /// under, lower first.
    pub(crate) under: (u64, u64),
}

impl Proof {
    pub(crate) fn fork(first: Signed<Header>, second: Signed<Header>) -> Self {
        Self::Fork(Box::new([first, second]))
    }

    /// Checks the proof on its own: both halves are signed by the same
    /// key, and no honest holder of it signs both. Whether that
    /// matters to `group` takes the chain to tell.
    pub(crate) fn check(&self, group: GroupId) -> Option<Offence> {
        match self {
            Self::Fork(headers) => forked(&headers[0], &headers[1], group).map(Offence::Fork),
            Self::Conflict(evidence) => evidence.check().map(Offence::Conflict),
        }
    }
}

/// Both headers are for the same place in `group`, and they differ.
fn forked(first: &Signed<Header>, second: &Signed<Header>, group: GroupId) -> Option<Fork> {
    let signed = vec![first.clone(), second.clone()];
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
