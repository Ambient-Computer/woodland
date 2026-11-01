use crate::quorum::Operation;

/// Orders competing attempts to commit during one epoch. Higher ballots
/// win.
///
/// Ordered by round, then proposer. Two proposers can pick the same round,
/// so the proposer's key keeps every ballot unique.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ballot<PublicKey> {
    pub round: u64,
    pub proposer: PublicKey,
}

/// An operation put forward under a ballot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal<PublicKey> {
    pub ballot: Ballot<PublicKey>,
    pub operation: Operation<PublicKey>,
}

/// Phase 1a: a proposer asks contributors to ignore lower ballots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepare<PublicKey> {
    pub epoch: u64,
    pub ballot: Ballot<PublicKey>,
}

/// Phase 1b: a contributor agrees to ignore ballots lower than `ballot`.
///
/// Carries the last proposal it accepted, if any. A proposer must adopt the
/// highest one it hears about instead of its own operation, which is how an
/// operation that may already have won is never replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Promise<PublicKey> {
    pub epoch: u64,
    pub ballot: Ballot<PublicKey>,
    pub accepted: Option<Proposal<PublicKey>>,
}

/// Phase 2a: after a majority promised, the proposer asks them to accept a
/// operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accept<PublicKey> {
    pub epoch: u64,
    pub proposal: Proposal<PublicKey>,
}

/// Phase 2b: a contributor accepted a proposal.
///
/// Sent to everyone, not just the proposer. Once a majority accepted the
/// same proposal, it's committed, and their signed `Accepted` messages
/// prove it to anyone who wasn't there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accepted<PublicKey> {
    pub epoch: u64,
    pub proposal: Proposal<PublicKey>,
}
