use signature::Verifier;

use super::Message;
use crate::envelope::Envelope;
use crate::quorum::Ledger;

/// Why a message wasn't accepted from its sender.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid signature")]
    InvalidSignature(#[source] signature::Error),

    /// The epoch hasn't started here yet. The sender may be ahead, so it's
    /// worth syncing commits from them.
    #[error("epoch has not started")]
    UnknownEpoch,

    #[error("sender is not a contributor during the message's epoch")]
    NotContributor,

    #[error("sender is not the ballot's proposer")]
    NotProposer,
}

impl<PublicKey: Ord + Clone, Signature> Ledger<PublicKey, Signature> {
    /// Checks that a Paxos message came from someone allowed to send it,
    /// then hands it over.
    ///
    /// Votes only count if each comes from a contributor during the epoch
    /// being decided. Membership changes between epochs, so a key that's a
    /// contributor now may not have been one then, or the other way around.
    ///
    /// Says nothing about whether the message is still relevant, e.g. for
    /// an epoch that already committed. That's for the caller to decide.
    ///
    /// # Errors
    ///
    /// Fails if the signature is invalid, the epoch hasn't started here,
    /// the sender wasn't a contributor during it, or the sender signed a
    /// message only the ballot's proposer may send.
    pub fn verify<'a, M: Message<PublicKey>>(
        &self,
        envelope: &'a Envelope<PublicKey, Signature, M>,
    ) -> Result<&'a M, Error>
    where
        PublicKey: Verifier<Signature>,
    {
        let message = envelope
            .verify(&self.id())
            .map_err(Error::InvalidSignature)?;

        let state = self.state(message.epoch()).ok_or(Error::UnknownEpoch)?;

        if !state.is_contributor(&envelope.author) {
            return Err(Error::NotContributor);
        }

        if message
            .required_signer()
            .is_some_and(|signer| signer != &envelope.author)
        {
            return Err(Error::NotProposer);
        }

        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ed25519_dalek::Signature;

    use super::*;
    use crate::quorum::consensus::{Accepted, Ballot, Certificate, Prepare, Promise, Proposal};
    use crate::quorum::{ContributorChange, Operation};
    use crate::testing::{Actor, Ledger, PublicKey, keys, laptop, nas, phone, quorum};

    fn ballot(proposer: &Actor) -> Ballot<PublicKey> {
        Ballot {
            round: 1,
            proposer: proposer.key(),
        }
    }

    fn prepare(epoch: u64, proposer: &Actor) -> Prepare<PublicKey> {
        Prepare {
            epoch,
            ballot: ballot(proposer),
        }
    }

    fn promise(epoch: u64, proposer: &Actor) -> Promise<PublicKey> {
        Promise {
            epoch,
            ballot: ballot(proposer),
            accepted: None,
        }
    }

    fn remove(actor: &Actor) -> Proposal<PublicKey> {
        Proposal {
            ballot: ballot(&phone()),
            operation: Operation::Contributors(ContributorChange {
                added: keys(&[]),
                removed: keys(&[actor]),
            }),
        }
    }

    fn sign<M: Message<PublicKey>>(
        signer: &Actor,
        ledger: &Ledger,
        message: M,
    ) -> Result<Envelope<PublicKey, Signature, M>, signature::Error> {
        Envelope::sign(signer, &ledger.id(), message)
    }

    #[test]
    fn accepts_a_contributor() -> Result<(), signature::Error> {
        let ledger = quorum(&[&phone(), &laptop()]);
        let envelope = sign(&laptop(), &ledger, promise(0, &phone()))?;

        assert_eq!(ledger.verify(&envelope).ok(), Some(&promise(0, &phone())));
        Ok(())
    }

    #[test]
    fn rejects_another_quorum() -> Result<(), signature::Error> {
        let ours = quorum(&[&phone(), &laptop()]);
        let theirs = quorum(&[&phone(), &laptop()]);
        let envelope = sign(&laptop(), &theirs, promise(0, &phone()))?;

        assert!(matches!(
            ours.verify(&envelope),
            Err(Error::InvalidSignature(_))
        ));
        Ok(())
    }

    #[test]
    fn rejects_an_epoch_that_has_not_started() -> Result<(), signature::Error> {
        let ledger = quorum(&[&phone(), &laptop()]);
        let envelope = sign(&laptop(), &ledger, promise(1, &phone()))?;

        assert!(matches!(ledger.verify(&envelope), Err(Error::UnknownEpoch)));
        Ok(())
    }

    #[test]
    fn rejects_a_stranger() -> Result<(), signature::Error> {
        let ledger = quorum(&[&phone(), &laptop()]);
        let envelope = sign(&nas(), &ledger, promise(0, &phone()))?;

        assert!(matches!(
            ledger.verify(&envelope),
            Err(Error::NotContributor)
        ));
        Ok(())
    }

    #[test]
    fn checks_membership_at_the_message_epoch() -> Result<(), signature::Error> {
        let mut ledger = quorum(&[&phone(), &laptop(), &nas()]);
        ledger.append(Certificate {
            epoch: 0,
            proposal: remove(&nas()),
            signatures: BTreeMap::new(),
        });

        let before = sign(
            &nas(),
            &ledger,
            Accepted {
                epoch: 0,
                proposal: remove(&nas()),
            },
        )?;
        let after = sign(&nas(), &ledger, promise(1, &phone()))?;

        assert!(ledger.verify(&before).is_ok());
        assert!(matches!(ledger.verify(&after), Err(Error::NotContributor)));
        Ok(())
    }

    #[test]
    fn only_the_proposer_asks_for_votes() -> Result<(), signature::Error> {
        let ledger = quorum(&[&phone(), &laptop()]);
        let own = sign(&phone(), &ledger, prepare(0, &phone()))?;
        let forged = sign(&laptop(), &ledger, prepare(0, &phone()))?;

        assert!(ledger.verify(&own).is_ok());
        assert!(matches!(ledger.verify(&forged), Err(Error::NotProposer)));
        Ok(())
    }
}
