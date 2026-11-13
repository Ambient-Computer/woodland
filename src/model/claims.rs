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
//! # Locks
//!
//! A claim can be given up, by a [release] that names what the key was
//! claimed for, and then claimed again. That makes a claim a lock that
//! works across transactions: claim a key for a token nobody else has,
//! make every transaction that needs the lock [expect] the key to hold
//! that token, and release it when done. Transactions that expected a
//! lock their device turned out not to get are pruned, like the claim
//! that didn't get it.
//!
//! [release]: crate::TransactionBuilder::release
//! [expect]: crate::TransactionBuilder::expect
//! [requires]: Requirement
//! [`Query::committed`]: crate::Query::committed

use serde::{Deserialize, Serialize};

use super::{Fingerprint, Requirement};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Operation {
    Claim {
        key: Vec<u8>,
        value: Vec<u8>,
    },
    /// Gives a key up. Only what it was claimed for can.
    Release {
        key: Vec<u8>,
        value: Vec<u8>,
    },
}

impl Operation {
    /// The operation's key, and what it holds once the operation is
    /// applied.
    pub(crate) fn write(&self) -> (Vec<u8>, Option<Vec<u8>>) {
        match self {
            Self::Claim { key, value } => (key.clone(), Some(value.clone())),
            Self::Release { key, .. } => (key.clone(), None),
        }
    }

    /// A claim needs its key to be vacant. A release needs it to be
    /// claimed for the value it names.
    pub(crate) fn requires(&self) -> (Vec<u8>, Requirement) {
        match self {
            Self::Claim { key, .. } => (key.clone(), Requirement::Vacant),
            Self::Release { key, value } => (key.clone(), Requirement::Is(Fingerprint::of(value))),
        }
    }
}
