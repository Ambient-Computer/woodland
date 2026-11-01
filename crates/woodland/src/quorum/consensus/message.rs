use std::collections::BTreeMap;

use crate::content_hash::{PayloadType, hasher_for, update_key};
use crate::envelope::Payload;
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
///
/// Signed by the ballot's proposer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepare<PublicKey> {
    pub epoch: u64,
    pub ballot: Ballot<PublicKey>,
}

/// Phase 1b: a contributor agrees to ignore ballots lower than `ballot`.
///
/// Signed by the contributor making the promise.
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

/// Phase 2a: after a majority promised, the proposer asks them to accept an
/// operation.
///
/// Signed by the ballot's proposer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accept<PublicKey> {
    pub epoch: u64,
    pub proposal: Proposal<PublicKey>,
}

/// Phase 2b: a contributor accepted a proposal.
///
/// Signed by the contributor that accepted.
///
/// Sent to everyone, not just the proposer. Once a majority accepted the
/// same proposal, it's committed, and their signed `Accepted` messages
/// prove it to anyone who wasn't there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accepted<PublicKey> {
    pub epoch: u64,
    pub proposal: Proposal<PublicKey>,
}

impl<PublicKey: AsRef<[u8]>> Payload for Prepare<PublicKey> {
    fn digest(&self) -> blake3::Hash {
        let mut hasher = hasher_for(PayloadType::Prepare);
        hasher.update(&self.epoch.to_le_bytes());
        update_ballot(&mut hasher, &self.ballot);
        hasher.finalize()
    }
}

impl<PublicKey: AsRef<[u8]>> Payload for Promise<PublicKey> {
    fn digest(&self) -> blake3::Hash {
        let mut hasher = hasher_for(PayloadType::Promise);
        hasher.update(&self.epoch.to_le_bytes());
        update_ballot(&mut hasher, &self.ballot);
        match &self.accepted {
            None => {
                hasher.update(&[0]);
            }
            Some(proposal) => {
                hasher.update(&[1]);
                update_proposal(&mut hasher, proposal);
            }
        }
        hasher.finalize()
    }
}

impl<PublicKey: AsRef<[u8]>> Payload for Accept<PublicKey> {
    fn digest(&self) -> blake3::Hash {
        let mut hasher = hasher_for(PayloadType::Accept);
        hasher.update(&self.epoch.to_le_bytes());
        update_proposal(&mut hasher, &self.proposal);
        hasher.finalize()
    }
}

impl<PublicKey: AsRef<[u8]>> Payload for Accepted<PublicKey> {
    fn digest(&self) -> blake3::Hash {
        let mut hasher = hasher_for(PayloadType::Accepted);
        hasher.update(&self.epoch.to_le_bytes());
        update_proposal(&mut hasher, &self.proposal);
        hasher.finalize()
    }
}

fn update_ballot(hasher: &mut blake3::Hasher, ballot: &Ballot<impl AsRef<[u8]>>) {
    hasher.update(&ballot.round.to_le_bytes());
    update_key(hasher, &ballot.proposer);
}

// The operation's digest already covers every field and has a fixed size,
// so it stands in for the operation.
fn update_proposal(hasher: &mut blake3::Hasher, proposal: &Proposal<impl AsRef<[u8]>>) {
    update_ballot(hasher, &proposal.ballot);
    hasher.update(proposal.operation.digest().as_bytes());
}

/// Proof that an operation committed: signed `Accepted` messages from a
/// strict majority of the epoch's contributors, all for the same proposal.
///
/// Persisted alongside the commit. Peers that weren't part of the vote check
/// it during sync instead of trusting whoever sent the commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate<PublicKey, Signature> {
    pub epoch: u64,
    pub proposal: Proposal<PublicKey>,

    /// Each contributor's signature on `Accepted { epoch, proposal }`.
    /// Every vote signs the same message, so only the signatures are kept.
    pub signatures: BTreeMap<PublicKey, Signature>,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::quorum::ContributorChange;

    fn ballot(round: u64, proposer: &'static str) -> Ballot<&'static str> {
        Ballot { round, proposer }
    }

    fn proposal(round: u64, added: &'static str) -> Proposal<&'static str> {
        Proposal {
            ballot: ballot(round, "phone"),
            operation: Operation::Contributors(ContributorChange {
                added: BTreeSet::from([added]),
                removed: BTreeSet::new(),
            }),
        }
    }

    fn prepare(epoch: u64, ballot: Ballot<&'static str>) -> Prepare<&'static str> {
        Prepare { epoch, ballot }
    }

    #[test]
    fn prepare_digest_covers_every_field() {
        let base = prepare(1, ballot(1, "phone")).digest();

        assert_ne!(base, prepare(2, ballot(1, "phone")).digest());
        assert_ne!(base, prepare(1, ballot(2, "phone")).digest());
        assert_ne!(base, prepare(1, ballot(1, "laptop")).digest());
    }

    #[test]
    fn promise_digest_covers_every_field() {
        let promise = |epoch, round, accepted| Promise {
            epoch,
            ballot: ballot(round, "laptop"),
            accepted,
        };
        let base = promise(1, 2, Some(proposal(1, "nas"))).digest();

        assert_ne!(base, promise(2, 2, Some(proposal(1, "nas"))).digest());
        assert_ne!(base, promise(1, 3, Some(proposal(1, "nas"))).digest());
        assert_ne!(base, promise(1, 2, Some(proposal(0, "nas"))).digest());
        assert_ne!(base, promise(1, 2, Some(proposal(1, "laptop"))).digest());
        assert_ne!(base, promise(1, 2, None).digest());
    }

    #[test]
    fn accept_digest_covers_every_field() {
        let accept = |epoch, proposal| Accept { epoch, proposal };
        let base = accept(1, proposal(1, "nas")).digest();

        assert_ne!(base, accept(2, proposal(1, "nas")).digest());
        assert_ne!(base, accept(1, proposal(2, "nas")).digest());
        assert_ne!(base, accept(1, proposal(1, "laptop")).digest());
    }

    #[test]
    fn accepted_digest_covers_every_field() {
        let accepted = |epoch, proposal| Accepted { epoch, proposal };
        let base = accepted(1, proposal(1, "nas")).digest();

        assert_ne!(base, accepted(2, proposal(1, "nas")).digest());
        assert_ne!(base, accepted(1, proposal(2, "nas")).digest());
        assert_ne!(base, accepted(1, proposal(1, "laptop")).digest());
    }

    // `Accept` and `Accepted` have identical fields. Without distinct type
    // tags, a proposer's request would double as a vote.
    #[test]
    fn accept_and_accepted_differ() {
        let accept = Accept {
            epoch: 1,
            proposal: proposal(1, "nas"),
        };
        let accepted = Accepted {
            epoch: 1,
            proposal: proposal(1, "nas"),
        };

        assert_ne!(accept.digest(), accepted.digest());
    }
}
