use std::collections::BTreeMap;

use signature::Verifier;

use super::{Accepted, Proposal};
use crate::envelope::signed_bytes;
use crate::quorum::{self, Ledger};

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

/// Why a certificate doesn't prove its operation committed.
#[derive(Debug, thiserror::Error)]
pub enum InvalidCertificate {
    /// The epoch hasn't started here yet. Commits before it are missing.
    #[error("epoch has not started")]
    UnknownEpoch,

    #[error("operation does not fit the epoch")]
    InvalidOperation(#[source] quorum::Error),

    #[error("signer is not a contributor during the epoch")]
    NotContributor,

    #[error("signers are not a majority of the epoch's contributors")]
    NoMajority,

    #[error("invalid signature")]
    InvalidSignature(#[source] signature::Error),
}

impl<PublicKey: Ord + Clone + AsRef<[u8]>, Signature> Certificate<PublicKey, Signature> {
    /// Checks that a majority of the epoch's contributors accepted this
    /// proposal.
    ///
    /// Also checks the operation fits the epoch, like everyone who receives
    /// an operation does. Contributors shouldn't accept an operation that
    /// doesn't fit, but a certificate is what's left to check for peers
    /// that weren't there.
    ///
    /// Rejects the whole certificate if any signature is bad, even if the
    /// rest would be a majority. Honest certificates only hold verified
    /// votes, so a bad one means someone tampered with it.
    ///
    /// # Errors
    ///
    /// Fails if the epoch hasn't started here, the operation doesn't fit
    /// it, a signer wasn't a contributor during it, the signers aren't a
    /// majority, or a signature is invalid.
    pub fn verify(&self, ledger: &Ledger<PublicKey, Signature>) -> Result<(), InvalidCertificate>
    where
        PublicKey: Verifier<Signature>,
    {
        let state = ledger
            .state(self.epoch)
            .ok_or(InvalidCertificate::UnknownEpoch)?;

        self.proposal
            .operation
            .validate(state)
            .map_err(InvalidCertificate::InvalidOperation)?;

        if !self
            .signatures
            .keys()
            .all(|signer| state.is_contributor(signer))
        {
            return Err(InvalidCertificate::NotContributor);
        }

        if !state.is_majority(self.signatures.len()) {
            return Err(InvalidCertificate::NoMajority);
        }

        let accepted = signed_bytes(
            &ledger.id(),
            &Accepted {
                epoch: self.epoch,
                proposal: self.proposal.clone(),
            },
        );

        for (signer, signature) in &self.signatures {
            signer
                .verify(&accepted, signature)
                .map_err(InvalidCertificate::InvalidSignature)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::Signature;

    use super::*;
    use crate::envelope::Envelope;
    use crate::quorum::consensus::Ballot;
    use crate::quorum::{ContributorChange, Operation};
    use crate::testing::{Actor, Ledger, PublicKey, keys, laptop, nas, phone, quorum};

    type Certificate = super::Certificate<PublicKey, Signature>;

    fn proposal(added: &[&Actor], removed: &[&Actor]) -> Proposal<PublicKey> {
        Proposal {
            ballot: Ballot {
                round: 1,
                proposer: phone().key(),
            },
            operation: Operation::Contributors(ContributorChange {
                added: keys(added),
                removed: keys(removed),
            }),
        }
    }

    fn add(actor: &Actor) -> Proposal<PublicKey> {
        proposal(&[actor], &[])
    }

    fn remove(actor: &Actor) -> Proposal<PublicKey> {
        proposal(&[], &[actor])
    }

    // Votes the way contributors do: by signing `Accepted`.
    fn certify(
        ledger: &Ledger,
        proposal: &Proposal<PublicKey>,
        voters: &[&Actor],
    ) -> Result<Certificate, signature::Error> {
        let accepted = Accepted {
            epoch: ledger.epoch(),
            proposal: proposal.clone(),
        };
        let signatures = voters
            .iter()
            .map(|voter| {
                let envelope = Envelope::sign(*voter, &ledger.id(), accepted.clone())?;
                Ok((envelope.author, envelope.signature))
            })
            .collect::<Result<_, signature::Error>>()?;

        Ok(Certificate {
            epoch: ledger.epoch(),
            proposal: proposal.clone(),
            signatures,
        })
    }

    #[test]
    fn accepts_a_majority() -> Result<(), signature::Error> {
        let ledger = quorum(&[&phone(), &laptop(), &nas()]);
        let certificate = certify(&ledger, &remove(&nas()), &[&phone(), &nas()])?;

        assert!(certificate.verify(&ledger).is_ok());
        Ok(())
    }

    #[test]
    fn rejects_half() -> Result<(), signature::Error> {
        let ledger = quorum(&[&phone(), &laptop()]);
        let certificate = certify(&ledger, &add(&nas()), &[&phone()])?;

        assert!(matches!(
            certificate.verify(&ledger),
            Err(InvalidCertificate::NoMajority)
        ));
        Ok(())
    }

    #[test]
    fn rejects_a_stranger() -> Result<(), signature::Error> {
        let ledger = quorum(&[&phone(), &laptop()]);
        let certificate = certify(&ledger, &add(&nas()), &[&phone(), &laptop(), &nas()])?;

        assert!(matches!(
            certificate.verify(&ledger),
            Err(InvalidCertificate::NotContributor)
        ));
        Ok(())
    }

    #[test]
    fn rejects_votes_for_another_proposal() -> Result<(), signature::Error> {
        let ledger = quorum(&[&phone(), &laptop()]);
        let mut certificate = certify(&ledger, &add(&nas()), &[&phone(), &laptop()])?;
        certificate.proposal = remove(&laptop());

        assert!(matches!(
            certificate.verify(&ledger),
            Err(InvalidCertificate::InvalidSignature(_))
        ));
        Ok(())
    }

    #[test]
    fn rejects_votes_from_another_quorum() -> Result<(), signature::Error> {
        let ours = quorum(&[&phone(), &laptop()]);
        let theirs = quorum(&[&phone(), &laptop()]);
        let certificate = certify(&theirs, &add(&nas()), &[&phone(), &laptop()])?;

        assert!(matches!(
            certificate.verify(&ours),
            Err(InvalidCertificate::InvalidSignature(_))
        ));
        Ok(())
    }

    #[test]
    fn rejects_an_operation_that_does_not_fit() -> Result<(), signature::Error> {
        let ledger = quorum(&[&phone(), &laptop()]);
        let certificate = certify(&ledger, &add(&laptop()), &[&phone(), &laptop()])?;

        assert!(matches!(
            certificate.verify(&ledger),
            Err(InvalidCertificate::InvalidOperation(_))
        ));
        Ok(())
    }

    #[test]
    fn rejects_an_epoch_that_has_not_started() -> Result<(), signature::Error> {
        let ledger = quorum(&[&phone(), &laptop()]);
        let mut certificate = certify(&ledger, &add(&nas()), &[&phone(), &laptop()])?;
        certificate.epoch = 1;

        assert!(matches!(
            certificate.verify(&ledger),
            Err(InvalidCertificate::UnknownEpoch)
        ));
        Ok(())
    }
}
