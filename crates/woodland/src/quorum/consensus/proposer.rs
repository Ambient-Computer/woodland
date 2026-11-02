use std::collections::BTreeMap;

use signature::Verifier;

#[cfg(doc)]
use super::Tally;
use super::{Accept, Ballot, Error, Prepare, Promise, Proposal, Rejected};
use crate::envelope::Envelope;
use crate::quorum::{self, Ledger, Operation};

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

    /// Not persisted. Votes can be lost safely, but must then be collected
    /// again from each contributor.
    pub phase: Phase<PublicKey>,
}

/// Where a proposer is in the protocol. Votes are keyed by the signer of a
/// verified message, so each contributor counts once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase<PublicKey> {
    /// Waiting for a majority to promise. Holds what each contributor
    /// reported having accepted.
    Preparing {
        promises: BTreeMap<PublicKey, Option<Proposal<PublicKey>>>,
    },

    /// Waiting for a majority to accept. Every contributor counts votes in
    /// its own [`Tally`], the proposer included, so there's nothing to hold
    /// here.
    Accepting,
}

impl<PublicKey: Ord + Clone + AsRef<[u8]>> Proposer<PublicKey> {
    /// Starts proposing `operation` for the ledger's current epoch, as
    /// `proposer`. Send [`prepare`](Self::prepare) to every contributor.
    ///
    /// Start at most one per epoch. Each starts at round 1, so a second
    /// would ask for a different operation under the same ballot.
    ///
    /// # Errors
    ///
    /// Fails if the operation doesn't fit the current epoch. Everyone else
    /// would refuse it anyway.
    pub fn new<Signature>(
        ledger: &Ledger<PublicKey, Signature>,
        proposer: PublicKey,
        operation: Operation<PublicKey>,
    ) -> Result<Self, quorum::Error> {
        operation.validate(ledger.current())?;

        Ok(Self {
            epoch: ledger.epoch(),
            ballot: Ballot { round: 1, proposer },
            operation,
            phase: Phase::Preparing {
                promises: BTreeMap::new(),
            },
        })
    }

    /// Asks contributors to promise this ballot.
    ///
    /// Keep advertising it until the epoch commits or a contributor rejects
    /// it. Offline, reaching a majority may take weeks, so there's no point
    /// at which silence means failure.
    #[must_use]
    pub fn prepare(&self) -> Prepare<PublicKey> {
        Prepare {
            epoch: self.epoch,
            ballot: self.ballot.clone(),
        }
    }

    /// Asks contributors to accept the operation, once a majority promised.
    /// `None` while still preparing.
    ///
    /// Like [`prepare`](Self::prepare), keep advertising it until the epoch
    /// commits or a contributor rejects it.
    #[must_use]
    pub fn accept(&self) -> Option<Accept<PublicKey>> {
        match self.phase {
            Phase::Preparing { .. } => None,
            Phase::Accepting => Some(Accept {
                epoch: self.epoch,
                proposal: Proposal {
                    ballot: self.ballot.clone(),
                    operation: self.operation.clone(),
                },
            }),
        }
    }

    /// Verifies a promise and counts it. Once a majority promised, adopts
    /// the highest proposal any of them accepted, then returns the `Accept`
    /// to send. Persist the proposer before sending it.
    ///
    /// Adopting is what keeps Paxos safe: an operation a majority may have
    /// accepted can't be replaced by a later ballot, only carried forward.
    ///
    /// Returns `None` for promises that change nothing: duplicates, replies
    /// to an earlier ballot or epoch, and promises after the majority.
    ///
    /// # Errors
    ///
    /// Fails if the promise doesn't verify against `ledger`, or if it's for
    /// a later epoch (see [`Error::StaleProposer`]).
    pub fn promise<Signature>(
        &mut self,
        ledger: &Ledger<PublicKey, Signature>,
        promise: &Envelope<PublicKey, Signature, Promise<PublicKey>>,
    ) -> Result<Option<Accept<PublicKey>>, Error>
    where
        PublicKey: Verifier<Signature>,
    {
        let message = ledger.verify(promise)?;

        // Same reasoning as the tally: the ledger moved on without us.
        if message.epoch > self.epoch {
            return Err(Error::StaleProposer);
        }

        if message.epoch < self.epoch || message.ballot != self.ballot {
            return Ok(None);
        }

        let Phase::Preparing { promises } = &mut self.phase else {
            return Ok(None);
        };

        promises.insert(promise.author.clone(), message.accepted.clone());

        let state = ledger.state(self.epoch).ok_or(Error::UnknownEpoch)?;
        if !state.is_majority(promises.len()) {
            return Ok(None);
        }

        let adopted = promises
            .values()
            .flatten()
            .max_by(|a, b| a.ballot.cmp(&b.ballot));

        if let Some(adopted) = adopted {
            self.operation = adopted.operation.clone();
        }

        self.phase = Phase::Accepting;
        Ok(self.accept())
    }

    // TODO: A WINNER THAT NEVER RETURNS STALLS THE EPOCH FOREVER.
    //
    // Everyone it outbid has yielded, and nothing raises a round any more,
    // so nobody takes over. Offline for weeks is fine: it resumes when it's
    // back. Gone for good (a lost phone, a wiped browser) is not. Taking
    // over needs a signal that comes from state, not a clock, e.g. the
    // winner being removed from the contributors. Needs real design work.

    /// Verifies a rejection, and whether it outbid this ballot. If so, the
    /// proposer yields: stop advertising and drop it, persisted state
    /// included.
    ///
    /// Yielding is how dueling proposers settle without clocks. Retrying
    /// above the winner would just invite it to retry above us, forever.
    /// Instead, the higher ballot gets the floor. Our operation isn't lost
    /// if a majority may have accepted it: the winner adopts it from the
    /// promises. Otherwise, propose it again next epoch.
    ///
    /// Proposals as a state-based CRDT could keep both sides' changes:
    /// instead of yielding, merge the winner's intent into ours and duel
    /// again above it. The winner would then see a proposal that already
    /// includes its own changes, and stop competing. In theory, that
    /// converges.
    ///
    /// One rejection is enough. That contributor will refuse this ballot
    /// for good, and someone with a higher ballot is competing for the
    /// epoch.
    ///
    /// Returns `false` for rejections that change nothing: replies to an
    /// earlier ballot or epoch, or one naming a ballot that doesn't beat
    /// this one.
    ///
    /// # Errors
    ///
    /// Fails if the rejection doesn't verify against `ledger`, or if it's
    /// for a later epoch (see [`Error::StaleProposer`]).
    pub fn outbid<Signature>(
        &self,
        ledger: &Ledger<PublicKey, Signature>,
        rejected: &Envelope<PublicKey, Signature, Rejected<PublicKey>>,
    ) -> Result<bool, Error>
    where
        PublicKey: Verifier<Signature>,
    {
        let message = ledger.verify(rejected)?;

        if message.epoch > self.epoch {
            return Err(Error::StaleProposer);
        }

        // An honest contributor only rejects for a higher ballot. Anything
        // else would make the proposer drop out for no reason.
        Ok(message.epoch == self.epoch
            && message.ballot == self.ballot
            && message.promised > self.ballot)
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::Signature;

    use super::*;
    use crate::quorum::ContributorChange;
    use crate::quorum::consensus::{Acceptor, Certificate, Refusal, Tally};
    use crate::testing::{Actor, Ledger, PublicKey, keys, laptop, nas, phone, quorum};

    type Proposer = super::Proposer<PublicKey>;

    fn remove(actor: &Actor) -> Operation<PublicKey> {
        Operation::Contributors(ContributorChange {
            added: keys(&[]),
            removed: keys(&[actor]),
        })
    }

    fn promise(
        ledger: &Ledger,
        acceptor: &Actor,
        ballot: &Ballot<PublicKey>,
        accepted: Option<Proposal<PublicKey>>,
    ) -> Result<Envelope<PublicKey, Signature, Promise<PublicKey>>, signature::Error> {
        let promise = Promise {
            epoch: ledger.epoch(),
            ballot: ballot.clone(),
            accepted,
        };

        Envelope::sign(acceptor, &ledger.id(), promise)
    }

    fn accepted(round: u64, operation: Operation<PublicKey>) -> Proposal<PublicKey> {
        Proposal {
            ballot: Ballot {
                round,
                proposer: laptop().key(),
            },
            operation,
        }
    }

    #[test]
    fn prepares_its_first_ballot() -> Result<(), quorum::Error> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let proposer = Proposer::new(&ledger, phone().key(), remove(&nas()))?;

        assert_eq!(
            proposer.prepare(),
            Prepare {
                epoch: 0,
                ballot: Ballot {
                    round: 1,
                    proposer: phone().key(),
                },
            }
        );
        assert_eq!(proposer.accept(), None);

        Ok(())
    }

    #[test]
    fn refuses_an_operation_that_does_not_fit() {
        let ledger = quorum(&[&phone(), &laptop()]);

        assert_eq!(
            Proposer::new(&ledger, phone().key(), remove(&nas())),
            Err(quorum::Error::NotContributor)
        );
    }

    #[test]
    fn asks_for_acceptance_at_a_majority() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut proposer = Proposer::new(&ledger, phone().key(), remove(&nas()))?;
        let ballot = proposer.ballot.clone();

        let first = proposer.promise(&ledger, &promise(&ledger, &phone(), &ballot, None)?)?;
        let second = proposer.promise(&ledger, &promise(&ledger, &laptop(), &ballot, None)?)?;

        assert_eq!(first, None);
        assert_eq!(
            second.map(|accept| accept.proposal),
            Some(Proposal {
                ballot,
                operation: remove(&nas()),
            })
        );

        Ok(())
    }

    #[test]
    fn adopts_the_highest_accepted_proposal() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut proposer = Proposer::new(&ledger, phone().key(), remove(&nas()))?;
        proposer.ballot.round = 3;
        let ballot = proposer.ballot.clone();

        let older = promise(
            &ledger,
            &laptop(),
            &ballot,
            Some(accepted(1, remove(&phone()))),
        )?;
        let newer = promise(
            &ledger,
            &nas(),
            &ballot,
            Some(accepted(2, remove(&laptop()))),
        )?;
        proposer.promise(&ledger, &older)?;
        let accept = proposer
            .promise(&ledger, &newer)?
            .ok_or("a majority promised")?;

        assert_eq!(accept.proposal.ballot, ballot);
        assert_eq!(accept.proposal.operation, remove(&laptop()));

        Ok(())
    }

    #[test]
    fn counts_each_promise_once() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut proposer = Proposer::new(&ledger, phone().key(), remove(&nas()))?;
        let ballot = proposer.ballot.clone();

        proposer.promise(&ledger, &promise(&ledger, &laptop(), &ballot, None)?)?;
        let again = proposer.promise(&ledger, &promise(&ledger, &laptop(), &ballot, None)?)?;

        assert_eq!(again, None);
        Ok(())
    }

    #[test]
    fn ignores_promises_for_another_ballot() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut proposer = Proposer::new(&ledger, phone().key(), remove(&nas()))?;
        let other = Ballot {
            round: 2,
            proposer: phone().key(),
        };

        proposer.promise(&ledger, &promise(&ledger, &phone(), &other, None)?)?;
        let late = proposer.promise(&ledger, &promise(&ledger, &laptop(), &other, None)?)?;

        assert_eq!(late, None);
        Ok(())
    }

    #[test]
    fn refuses_when_the_ledger_moved_on() -> Result<(), Box<dyn std::error::Error>> {
        let mut ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut stale = Proposer::new(&ledger, phone().key(), remove(&nas()))?;
        ledger.append(Certificate {
            epoch: 0,
            proposal: Proposal {
                ballot: stale.ballot.clone(),
                operation: remove(&nas()),
            },
            signatures: BTreeMap::new(),
        });
        let current = promise(&ledger, &laptop(), &stale.ballot, None)?;

        assert!(matches!(
            stale.promise(&ledger, &current),
            Err(Error::StaleProposer)
        ));

        Ok(())
    }

    fn rejected(
        ledger: &Ledger,
        acceptor: &Actor,
        ballot: &Ballot<PublicKey>,
        promised: Ballot<PublicKey>,
    ) -> Result<Envelope<PublicKey, Signature, Rejected<PublicKey>>, signature::Error> {
        let rejected = Rejected {
            epoch: ledger.epoch(),
            ballot: ballot.clone(),
            promised,
        };

        Envelope::sign(acceptor, &ledger.id(), rejected)
    }

    #[test]
    fn yields_to_a_higher_ballot() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let proposer = Proposer::new(&ledger, phone().key(), remove(&nas()))?;
        let winner = Ballot {
            round: 5,
            proposer: laptop().key(),
        };
        let rejection = rejected(&ledger, &nas(), &proposer.ballot, winner)?;

        assert!(proposer.outbid(&ledger, &rejection)?);
        Ok(())
    }

    #[test]
    fn ignores_rejections_that_change_nothing() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let proposer = Proposer::new(&ledger, phone().key(), remove(&nas()))?;
        let other = Ballot {
            round: 2,
            proposer: phone().key(),
        };
        let lower = Ballot {
            round: 0,
            proposer: laptop().key(),
        };
        let higher = Ballot {
            round: 5,
            proposer: laptop().key(),
        };

        let for_another_ballot = rejected(&ledger, &nas(), &other, higher)?;
        let not_outbid = rejected(&ledger, &nas(), &proposer.ballot, lower)?;

        assert!(!proposer.outbid(&ledger, &for_another_ballot)?);
        assert!(!proposer.outbid(&ledger, &not_outbid)?);

        Ok(())
    }

    // Dueling proposers: both start at round 1, so the ballots differ only
    // by proposer. Whoever every acceptor heard from first wins, and the
    // other yields instead of fighting back.
    #[test]
    fn duels_end_with_one_proposer() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let (winner, loser) = if ballot_of(&phone()) > ballot_of(&laptop()) {
            (phone(), laptop())
        } else {
            (laptop(), phone())
        };
        let winning = Proposer::new(&ledger, winner.key(), remove(&nas()))?;
        let losing = Proposer::new(&ledger, loser.key(), remove(&nas()))?;
        let winning_prepare = Envelope::sign(&winner, &ledger.id(), winning.prepare())?;
        let losing_prepare = Envelope::sign(&loser, &ledger.id(), losing.prepare())?;

        let mut acceptor = Acceptor::new(0);
        acceptor.prepare(&ledger, &winning_prepare)?;
        let Err(Refusal::Superseded(refused)) = acceptor.prepare(&ledger, &losing_prepare) else {
            return Err("a higher ballot was promised".into());
        };
        let rejection = Envelope::sign(&nas(), &ledger.id(), refused)?;

        assert!(losing.outbid(&ledger, &rejection)?);
        assert!(acceptor.prepare(&ledger, &winning_prepare).is_ok());

        Ok(())
    }

    fn ballot_of(proposer: &Actor) -> Ballot<PublicKey> {
        Ballot {
            round: 1,
            proposer: proposer.key(),
        }
    }

    // Every role together: a proposal reaches a majority and commits.
    #[test]
    fn commits_through_a_majority() -> Result<(), Box<dyn std::error::Error>> {
        let mut ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let voters = [phone(), laptop()];
        let mut acceptors = [Acceptor::new(0), Acceptor::new(0)];
        let mut tally = Tally::new(0);
        let mut proposer = Proposer::new(&ledger, phone().key(), remove(&nas()))?;

        let prepare = Envelope::sign(&phone(), &ledger.id(), proposer.prepare())?;
        let mut accept = None;
        for (voter, acceptor) in voters.iter().zip(&mut acceptors) {
            let promise = acceptor.prepare(&ledger, &prepare)?;
            accept = proposer.promise(&ledger, &Envelope::sign(voter, &ledger.id(), promise)?)?;
        }

        let accept = Envelope::sign(&phone(), &ledger.id(), accept.ok_or("a majority promised")?)?;
        let mut certificate = None;
        for (voter, acceptor) in voters.iter().zip(&mut acceptors) {
            let accepted = acceptor.accept(&ledger, &accept)?;
            certificate = tally.count(&ledger, Envelope::sign(voter, &ledger.id(), accepted)?)?;
        }

        let certificate = certificate.ok_or("a majority accepted")?;
        certificate.verify(&ledger)?;
        ledger.append(certificate);

        assert_eq!(
            ledger.current().contributors(),
            &keys(&[&phone(), &laptop()])
        );

        Ok(())
    }
}
