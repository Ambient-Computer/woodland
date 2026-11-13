//! Claims: a map where the first transaction to claim a key keeps it.
//!
//! It's for things only one device may have: a username, an invitation
//! that works once, a seat, the next task in a queue. A key-value map
//! can't do that. Its last writer wins, so a device that claims a key
//! while offline takes it from whoever had it.
//!
//! # The rule
//!
//! A claim [requires] its key to be vacant at its transaction's place in
//! the timeline. If it isn't, the transaction is pruned: none of its
//! operations happen, in this model or any other. So a transaction can
//! claim a username and write the profile that goes with it, and the
//! profile is only ever written by whoever got the name.
//!
//! # Who wins
//!
//! Among concurrent claims, the one that comes first in the timeline.
//! That's arbitrary, but the same on every device. A device sees its own
//! claim succeed until an earlier one arrives, which can happen for as
//! long as the claim hasn't settled. A committed read shows only the
//! claim that won for good (see [`Query::committed`]).
//!
//! [requires]: Requirement
//! [`Query::committed`]: crate::Query::committed

use serde::{Deserialize, Serialize};

use super::Requirement;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Operation {
    Claim { key: Vec<u8>, value: Vec<u8> },
}

impl Operation {
    /// The operation's key, and what it holds once the operation is
    /// applied.
    pub(crate) fn write(&self) -> (Vec<u8>, Option<Vec<u8>>) {
        let Self::Claim { key, value } = self;

        (key.clone(), Some(value.clone()))
    }

    /// Every claim has the same rule, about its own key.
    pub(crate) fn requires(&self) -> (Vec<u8>, Requirement) {
        let Self::Claim { key, .. } = self;

        (key.clone(), Requirement::Vacant)
    }
}
