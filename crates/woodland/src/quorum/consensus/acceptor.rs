use signature::Verifier;

use super::{Accept, Accepted, Ballot, Error, Prepare, Promise, Proposal, Rejected};
use crate::envelope::Envelope;
use crate::quorum::{self, Ledger};

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

/// Why an acceptor refused a message. Refusing is always safe: at worst,
/// the epoch waits for another proposer.
#[derive(Debug, thiserror::Error)]
pub enum Refusal<PublicKey> {
    #[error("message rejected")]
    Rejected(#[from] Error),

    /// The epoch already committed. The proposer is missing commits and
    /// should sync before trying again.
    #[error("epoch already committed")]
    EpochOver,

    /// A message for an epoch after the acceptor's, which the ledger has
    /// already reached. The caller kept an old acceptor after committing,
    /// instead of starting a new one.
    #[error("acceptor is for an earlier epoch than the ledger")]
    StaleAcceptor,

    /// A higher ballot was already promised. Sign the rejection and send it
    /// back, so the proposer knows to yield.
    #[error("ballot is lower than one already promised")]
    Superseded(Rejected<PublicKey>),

    #[error("operation does not fit the epoch")]
    InvalidOperation(#[source] quorum::Error),
}

impl<PublicKey: Ord + Clone + AsRef<[u8]>> Acceptor<PublicKey> {
    /// An acceptor that hasn't promised or accepted anything yet.
    #[must_use]
    pub fn new(epoch: u64) -> Self {
        Self {
            epoch,
            promised: None,
            accepted: None,
        }
    }

    /// Promises to ignore ballots lower than this one. Persist the acceptor
    /// before sending the promise.
    ///
    /// Repeating the current promise is allowed, so a proposer can safely
    /// resend `Prepare`.
    ///
    /// # Errors
    ///
    /// Fails if the message doesn't verify, isn't for this epoch, or a
    /// higher ballot was already promised.
    pub fn prepare<Signature>(
        &mut self,
        ledger: &Ledger<PublicKey, Signature>,
        prepare: &Envelope<PublicKey, Signature, Prepare<PublicKey>>,
    ) -> Result<Promise<PublicKey>, Refusal<PublicKey>>
    where
        PublicKey: Verifier<Signature>,
    {
        let prepare = ledger.verify(prepare)?;
        self.check(prepare.epoch, &prepare.ballot)?;
        self.promised = Some(prepare.ballot.clone());

        Ok(Promise {
            epoch: self.epoch,
            ballot: prepare.ballot.clone(),
            accepted: self.accepted.clone(),
        })
    }

    /// Accepts a proposal unless a higher ballot was promised. Persist the
    /// acceptor before sending the vote.
    ///
    /// Validates the operation first, like everyone who receives one.
    ///
    /// # Errors
    ///
    /// Fails if the message doesn't verify, isn't for this epoch, a higher
    /// ballot was already promised, or the operation doesn't fit the epoch.
    pub fn accept<Signature>(
        &mut self,
        ledger: &Ledger<PublicKey, Signature>,
        accept: &Envelope<PublicKey, Signature, Accept<PublicKey>>,
    ) -> Result<Accepted<PublicKey>, Refusal<PublicKey>>
    where
        PublicKey: Verifier<Signature>,
    {
        let accept = ledger.verify(accept)?;
        self.check(accept.epoch, &accept.proposal.ballot)?;

        let state = ledger.state(accept.epoch).ok_or(Error::UnknownEpoch)?;
        accept
            .proposal
            .operation
            .validate(state)
            .map_err(Refusal::InvalidOperation)?;

        self.promised = Some(accept.proposal.ballot.clone());
        self.accepted = Some(accept.proposal.clone());

        Ok(Accepted {
            epoch: self.epoch,
            proposal: accept.proposal.clone(),
        })
    }

    fn check(&self, epoch: u64, ballot: &Ballot<PublicKey>) -> Result<(), Refusal<PublicKey>> {
        // Verified messages are for an epoch the ledger has reached, so a
        // later one means this acceptor fell behind its own ledger.
        if epoch > self.epoch {
            return Err(Refusal::StaleAcceptor);
        }

        if epoch < self.epoch {
            return Err(Refusal::EpochOver);
        }

        match &self.promised {
            Some(promised) if ballot < promised => Err(Refusal::Superseded(Rejected {
                epoch,
                ballot: ballot.clone(),
                promised: promised.clone(),
            })),

            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ed25519_dalek::Signature;

    use super::*;
    use crate::quorum::consensus::Certificate;
    use crate::quorum::{ContributorChange, Operation};
    use crate::testing::{Actor, Ledger, PublicKey, keys, laptop, nas, phone, quorum};

    type Acceptor = super::Acceptor<PublicKey>;

    fn ballot(round: u64, proposer: &Actor) -> Ballot<PublicKey> {
        Ballot {
            round,
            proposer: proposer.key(),
        }
    }

    fn remove(round: u64, proposer: &Actor, removed: &Actor) -> Proposal<PublicKey> {
        Proposal {
            ballot: ballot(round, proposer),
            operation: Operation::Contributors(ContributorChange {
                added: keys(&[]),
                removed: keys(&[removed]),
            }),
        }
    }

    fn prepare(
        ledger: &Ledger,
        round: u64,
        proposer: &Actor,
    ) -> Result<Envelope<PublicKey, Signature, Prepare<PublicKey>>, signature::Error> {
        let prepare = Prepare {
            epoch: ledger.epoch(),
            ballot: ballot(round, proposer),
        };

        Envelope::sign(proposer, &ledger.id(), prepare)
    }

    fn accept(
        ledger: &Ledger,
        proposer: &Actor,
        proposal: Proposal<PublicKey>,
    ) -> Result<Envelope<PublicKey, Signature, Accept<PublicKey>>, signature::Error> {
        let accept = Accept {
            epoch: ledger.epoch(),
            proposal,
        };

        Envelope::sign(proposer, &ledger.id(), accept)
    }

    // Commits without votes: the ledger trusts what it's given.
    fn commit(ledger: &mut Ledger, proposal: Proposal<PublicKey>) {
        ledger.append(Certificate {
            epoch: ledger.epoch(),
            proposal,
            signatures: BTreeMap::new(),
        });
    }

    #[test]
    fn promises_the_first_ballot() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut acceptor = Acceptor::new(0);
        let promise = acceptor.prepare(&ledger, &prepare(&ledger, 1, &phone())?)?;

        assert_eq!(promise.ballot, ballot(1, &phone()));
        assert_eq!(promise.accepted, None);
        assert_eq!(acceptor.promised, Some(ballot(1, &phone())));
        Ok(())
    }

    #[test]
    fn promises_higher_ballots() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut acceptor = Acceptor::new(0);
        acceptor.prepare(&ledger, &prepare(&ledger, 1, &phone())?)?;
        acceptor.prepare(&ledger, &prepare(&ledger, 2, &laptop())?)?;

        assert_eq!(acceptor.promised, Some(ballot(2, &laptop())));
        Ok(())
    }

    #[test]
    fn repeats_the_current_promise() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut acceptor = Acceptor::new(0);
        acceptor.prepare(&ledger, &prepare(&ledger, 1, &phone())?)?;

        assert!(
            acceptor
                .prepare(&ledger, &prepare(&ledger, 1, &phone())?)
                .is_ok()
        );

        Ok(())
    }

    #[test]
    fn breaks_round_ties_by_proposer() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let (low, high) = if ballot(1, &phone()) < ballot(1, &laptop()) {
            (phone(), laptop())
        } else {
            (laptop(), phone())
        };

        let mut acceptor = Acceptor::new(0);
        acceptor.prepare(&ledger, &prepare(&ledger, 1, &high)?)?;

        assert!(matches!(
            acceptor.prepare(&ledger, &prepare(&ledger, 1, &low)?),
            Err(Refusal::Superseded(_))
        ));

        Ok(())
    }

    #[test]
    fn refuses_lower_ballots() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut acceptor = Acceptor::new(0);
        acceptor.prepare(&ledger, &prepare(&ledger, 2, &phone())?)?;

        assert!(matches!(
            acceptor.prepare(&ledger, &prepare(&ledger, 1, &laptop())?),
            Err(Refusal::Superseded(rejected)) if rejected == Rejected {
                epoch: 0,
                ballot: ballot(1, &laptop()),
                promised: ballot(2, &phone()),
            }
        ));
        assert!(matches!(
            acceptor.accept(
                &ledger,
                &accept(&ledger, &laptop(), remove(1, &laptop(), &nas()))?
            ),
            Err(Refusal::Superseded(_))
        ));

        Ok(())
    }

    #[test]
    fn promises_report_what_was_accepted() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut acceptor = Acceptor::new(0);
        let accepted = acceptor.accept(
            &ledger,
            &accept(&ledger, &phone(), remove(1, &phone(), &nas()))?,
        )?;
        let promise = acceptor.prepare(&ledger, &prepare(&ledger, 2, &laptop())?)?;

        assert_eq!(promise.accepted, Some(accepted.proposal));
        Ok(())
    }

    #[test]
    fn refuses_operations_that_do_not_fit() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop()]);
        let mut acceptor = Acceptor::new(0);
        let removes_a_stranger = accept(&ledger, &phone(), remove(1, &phone(), &nas()))?;

        assert!(matches!(
            acceptor.accept(&ledger, &removes_a_stranger),
            Err(Refusal::InvalidOperation(_))
        ));
        assert_eq!(acceptor.accepted, None);

        Ok(())
    }

    #[test]
    fn refuses_unverified_messages() -> Result<(), Box<dyn std::error::Error>> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut acceptor = Acceptor::new(0);
        let forged = Envelope::sign(
            &laptop(),
            &ledger.id(),
            Prepare {
                epoch: 0,
                ballot: ballot(1, &phone()),
            },
        )?;

        assert!(matches!(
            acceptor.prepare(&ledger, &forged),
            Err(Refusal::Rejected(Error::NotProposer))
        ));

        Ok(())
    }

    #[test]
    fn refuses_epochs_that_already_committed() -> Result<(), Box<dyn std::error::Error>> {
        let mut ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let late = prepare(&ledger, 1, &phone())?;
        commit(&mut ledger, remove(1, &laptop(), &nas()));

        assert!(matches!(
            Acceptor::new(1).prepare(&ledger, &late),
            Err(Refusal::EpochOver)
        ));

        Ok(())
    }

    #[test]
    fn refuses_when_the_ledger_moved_on() -> Result<(), Box<dyn std::error::Error>> {
        let mut ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let mut stale = Acceptor::new(0);
        commit(&mut ledger, remove(1, &laptop(), &nas()));

        assert!(matches!(
            stale.prepare(&ledger, &prepare(&ledger, 1, &phone())?),
            Err(Refusal::StaleAcceptor)
        ));

        Ok(())
    }
}
