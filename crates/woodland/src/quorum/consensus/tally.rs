use std::collections::BTreeMap;

use signature::Verifier;

use super::{Accepted, Ballot, Certificate, Error, Proposal};
use crate::envelope::Envelope;
use crate::quorum::Ledger;

/// Counts `Accepted` votes for one epoch until a proposal has a majority,
/// then turns them into the commit's [`Certificate`].
///
/// Every contributor keeps one, not just the proposer. Votes are sent to
/// everyone, so anyone can see the commit happen without waiting to hear
/// it from someone else.
///
/// Not persisted. Votes lost in a restart can be heard again, or the
/// commit learned from a peer's certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tally<PublicKey, Signature> {
    epoch: u64,

    /// Votes are counted per ballot. A contributor may accept several
    /// ballots in one epoch, but only votes for the same proposal add up.
    ballots: BTreeMap<Ballot<PublicKey>, Votes<PublicKey, Signature>>,

    certified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Votes<PublicKey, Signature> {
    proposal: Proposal<PublicKey>,
    signatures: BTreeMap<PublicKey, Signature>,
}

impl<PublicKey, Signature> Tally<PublicKey, Signature> {
    #[must_use]
    pub fn new(epoch: u64) -> Self {
        Self {
            epoch,
            ballots: BTreeMap::new(),
            certified: false,
        }
    }

    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

impl<PublicKey: Ord + Clone + AsRef<[u8]>, Signature: Clone> Tally<PublicKey, Signature> {
    /// Verifies a vote and counts it. Returns the certificate once, on the
    /// vote that gives a proposal its majority.
    ///
    /// Returns `None` for votes that are valid but change nothing. Votes go
    /// to everyone and may arrive more than once or late, so these are
    /// expected, not faults:
    ///
    /// - A vote heard before, from the same contributor on the same ballot.
    /// - A vote after the certificate was made. A majority already decided.
    /// - A vote for an earlier epoch, e.g. from a peer catching up.
    ///
    /// # Errors
    ///
    /// Fails if the vote doesn't verify against `ledger`, e.g. a signer who
    /// wasn't a contributor during the vote's epoch, or if the vote is for a
    /// later epoch (see [`Error::StaleTally`]).
    pub fn count(
        &mut self,
        ledger: &Ledger<PublicKey, Signature>,
        vote: Envelope<PublicKey, Signature, Accepted<PublicKey>>,
    ) -> Result<Option<Certificate<PublicKey, Signature>>, Error>
    where
        PublicKey: Verifier<Signature>,
    {
        let Accepted { epoch, proposal } = ledger.verify(&vote)?.clone();

        // The ledger has reached this epoch, so the vote is current and this
        // tally is not. Dropping it would silently stall the epoch.
        if epoch > self.epoch {
            return Err(Error::StaleTally);
        }

        // Late gossip: an epoch that's over, or a decision already made.
        if epoch < self.epoch || self.certified {
            return Ok(None);
        }

        let votes = self
            .ballots
            .entry(proposal.ballot.clone())
            .or_insert_with(|| Votes {
                proposal: proposal.clone(),
                signatures: BTreeMap::new(),
            });

        // A proposer never asks for two operations under one ballot, but
        // mixing them would make a certificate that doesn't verify.
        if votes.proposal != proposal {
            return Ok(None);
        }

        votes.signatures.insert(vote.author, vote.signature);

        let state = ledger.state(epoch).ok_or(Error::UnknownEpoch)?;
        if !state.is_majority(votes.signatures.len()) {
            return Ok(None);
        }

        self.certified = true;
        Ok(Some(Certificate {
            epoch,
            proposal,
            signatures: votes.signatures.clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::Signature;

    use super::*;
    use crate::quorum::{ContributorChange, Operation};
    use crate::testing::{Actor, Ledger, PublicKey, keys, laptop, nas, phone, quorum};

    type Tally = super::Tally<PublicKey, Signature>;
    type Vote = Envelope<PublicKey, Signature, Accepted<PublicKey>>;

    fn remove(round: u64, actor: &Actor) -> Proposal<PublicKey> {
        Proposal {
            ballot: Ballot {
                round,
                proposer: phone().key(),
            },
            operation: Operation::Contributors(ContributorChange {
                added: keys(&[]),
                removed: keys(&[actor]),
            }),
        }
    }

    fn vote(
        voter: &Actor,
        ledger: &Ledger,
        proposal: Proposal<PublicKey>,
    ) -> Result<Vote, signature::Error> {
        let accepted = Accepted {
            epoch: ledger.epoch(),
            proposal,
        };

        Envelope::sign(voter, &ledger.id(), accepted)
    }

    #[test]
    fn certifies_at_a_majority() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut tally = Tally::new(0);

        let first = tally.count(&ledger, vote(&phone(), &ledger, remove(1, &nas()))?)?;
        let second = tally.count(&ledger, vote(&laptop(), &ledger, remove(1, &nas()))?)?;

        assert_eq!(first, None);
        let certificate = second.ok_or("a majority voted")?;
        assert_eq!(certificate.proposal, remove(1, &nas()));
        certificate.verify(&ledger)?;
        Ok(())
    }

    #[test]
    fn counts_each_voter_once() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut tally = Tally::new(0);

        tally.count(&ledger, vote(&phone(), &ledger, remove(1, &nas()))?)?;
        let again = tally.count(&ledger, vote(&phone(), &ledger, remove(1, &nas()))?)?;

        assert_eq!(again, None);
        Ok(())
    }

    #[test]
    fn keeps_ballots_apart() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut tally = Tally::new(0);

        tally.count(&ledger, vote(&phone(), &ledger, remove(1, &nas()))?)?;
        let other = tally.count(&ledger, vote(&laptop(), &ledger, remove(2, &nas()))?)?;

        assert_eq!(other, None);
        Ok(())
    }

    #[test]
    fn certifies_once() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut tally = Tally::new(0);

        tally.count(&ledger, vote(&phone(), &ledger, remove(1, &nas()))?)?;
        tally.count(&ledger, vote(&laptop(), &ledger, remove(1, &nas()))?)?;
        let late = tally.count(&ledger, vote(&nas(), &ledger, remove(1, &nas()))?)?;

        assert_eq!(late, None);
        Ok(())
    }

    #[test]
    fn ignores_other_epochs() -> Result<(), Box<dyn std::error::Error>> {
        let mut ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut tally = Tally::new(0);
        let late = vote(&nas(), &ledger, remove(1, &nas()))?;

        tally.count(&ledger, vote(&phone(), &ledger, remove(1, &nas()))?)?;
        let certificate = tally
            .count(&ledger, vote(&laptop(), &ledger, remove(1, &nas()))?)?
            .ok_or("a majority voted")?;

        ledger.append(certificate);

        assert_eq!(Tally::new(1).count(&ledger, late)?, None);
        Ok(())
    }

    #[test]
    fn rejects_votes_for_a_later_epoch() -> Result<(), Box<dyn std::error::Error>> {
        let mut ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut stale = Tally::new(0);

        stale.count(&ledger, vote(&phone(), &ledger, remove(1, &nas()))?)?;
        let certificate = stale
            .count(&ledger, vote(&laptop(), &ledger, remove(1, &nas()))?)?
            .ok_or("a majority voted")?;

        ledger.append(certificate);
        let current = vote(&phone(), &ledger, remove(1, &laptop()))?;

        assert!(matches!(
            stale.count(&ledger, current),
            Err(Error::StaleTally)
        ));

        Ok(())
    }

    #[test]
    fn rejects_unverified_votes() -> Result<(), signature::Error> {
        let ledger = quorum(&[&phone(), &laptop()]);
        let mut tally = Tally::new(0);
        let stranger = vote(&nas(), &ledger, remove(1, &laptop()))?;

        assert!(matches!(
            tally.count(&ledger, stranger),
            Err(Error::NotContributor)
        ));

        Ok(())
    }
}
