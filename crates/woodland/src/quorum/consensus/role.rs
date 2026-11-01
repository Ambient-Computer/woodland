use std::collections::{BTreeMap, BTreeSet};

use super::{Ballot, Proposal};
use crate::quorum::Operation;

/// What a contributor has promised and accepted during one epoch.
///
/// Persisted. A contributor that forgets a promise could vote for two
/// different operations, letting two commits win the same epoch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Acceptor<PublicKey> {
    pub epoch: u64,

    /// The highest ballot it promised. Lower ballots are ignored.
    pub promised: Option<Ballot<PublicKey>>,

    /// The last proposal it accepted, reported in later promises.
    pub accepted: Option<Proposal<PublicKey>>,
}

/// A contributor's attempt to commit an operation during one epoch.
///
/// Persisted, except for `phase`. A proposer that forgets its ballot could
/// reuse it after a restart, asking for two different operations under the
/// same ballot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposer<PublicKey> {
    pub epoch: u64,
    pub ballot: Ballot<PublicKey>,

    /// The operation being proposed. Starts as the proposer's own, but may
    /// be replaced by one adopted from a promise.
    pub operation: Operation<PublicKey>,

    /// Not persisted. Votes can be lost safely; the proposer just starts
    /// over with a higher ballot.
    pub phase: Phase<PublicKey>,
}

/// Where a proposer is in the protocol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase<PublicKey> {
    /// Waiting for a majority to promise. Holds what each contributor
    /// reported having accepted.
    Preparing {
        promises: BTreeMap<PublicKey, Option<Proposal<PublicKey>>>,
    },

    /// Waiting for a majority to accept.
    Accepting { accepted: BTreeSet<PublicKey> },
}
