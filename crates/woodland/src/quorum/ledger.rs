use super::consensus::Certificate;
use super::{Initialize, State};

/// Every committed quorum operation, in order, each with the certificate
/// proving it was decided. The source of truth for quorum state: states are
/// derived from it, never stored on their own.
///
/// The epoch is the number of commits, and indexes the state that was
/// current at the time.
///
/// Persist `initial_state` and `commits`, then restore with
/// [`derive`](Self::derive). Everything else is rebuilt from them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ledger<PublicKey, Signature> {
    id: blake3::Hash,
    initial_state: Initialize<PublicKey>,
    commits: Vec<Certificate<PublicKey, Signature>>,

    /// One per epoch, so past epochs can be checked without replaying.
    states: Vec<State<PublicKey>>,
}

impl<PublicKey: Ord + Clone + AsRef<[u8]>, Signature> Ledger<PublicKey, Signature> {
    /// Replays commits on top of the initial state.
    pub fn derive(
        initial_state: Initialize<PublicKey>,
        commits: impl IntoIterator<Item = Certificate<PublicKey, Signature>>,
    ) -> Self {
        let ledger = Self {
            id: initial_state.id(),
            states: vec![State::initial(&initial_state)],
            initial_state,
            commits: Vec::new(),
        };

        commits.into_iter().fold(ledger, |mut ledger, commit| {
            ledger.append(commit);
            ledger
        })
    }

    /// Appends the next commit, starting a new epoch. Commits are validated
    /// long before they get here, when proposed and accepted, and their
    /// certificates when received.
    pub fn append(&mut self, commit: Certificate<PublicKey, Signature>) {
        self.states
            .push(self.current().merge(&commit.proposal.operation));

        self.commits.push(commit);
    }
}

impl<PublicKey, Signature> Ledger<PublicKey, Signature> {
    #[must_use]
    pub fn id(&self) -> blake3::Hash {
        self.id
    }

    #[must_use]
    pub fn initial_state(&self) -> &Initialize<PublicKey> {
        &self.initial_state
    }

    #[must_use]
    pub fn commits(&self) -> &[Certificate<PublicKey, Signature>] {
        &self.commits
    }

    /// The current epoch.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.commits.len() as u64
    }

    /// The state for the current epoch.
    #[must_use]
    pub fn current(&self) -> &State<PublicKey> {
        &self.states[self.commits.len()]
    }

    /// The state during `epoch`, or `None` if that epoch hasn't started.
    #[must_use]
    pub fn state(&self, epoch: u64) -> Option<&State<PublicKey>> {
        self.states.get(usize::try_from(epoch).ok()?)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use ed25519_dalek::Signature;

    use super::*;
    use crate::quorum::consensus::{Ballot, Proposal};
    use crate::quorum::{ContributorChange, Operation};

    // Commits carry no signatures here: the ledger trusts its commits.
    type Ledger = super::Ledger<&'static str, Signature>;

    fn keys(keys: &[&'static str]) -> BTreeSet<&'static str> {
        keys.iter().copied().collect()
    }

    fn initialize(contributors: &[&'static str]) -> Initialize<&'static str> {
        Initialize {
            controller: "controller",
            contributors: keys(contributors),
        }
    }

    fn commit(
        epoch: u64,
        added: &[&'static str],
        removed: &[&'static str],
    ) -> Certificate<&'static str, Signature> {
        Certificate {
            epoch,
            proposal: Proposal {
                ballot: Ballot {
                    round: 1,
                    proposer: "phone",
                },
                operation: Operation::Contributors(ContributorChange {
                    added: keys(added),
                    removed: keys(removed),
                }),
            },
            signatures: BTreeMap::new(),
        }
    }

    #[test]
    fn starts_at_epoch_zero() {
        let ledger = Ledger::derive(initialize(&["phone", "laptop"]), []);

        assert_eq!(ledger.epoch(), 0);
        assert_eq!(ledger.current().contributors(), &keys(&["phone", "laptop"]));
    }

    #[test]
    fn applies_commits_in_order() {
        let ledger = Ledger::derive(
            initialize(&["phone"]),
            [commit(0, &["laptop"], &[]), commit(1, &["nas"], &["phone"])],
        );

        assert_eq!(ledger.epoch(), 2);
        assert_eq!(ledger.current().contributors(), &keys(&["laptop", "nas"]));
    }

    #[test]
    fn keeps_past_states() {
        let ledger = Ledger::derive(initialize(&["phone"]), [commit(0, &["laptop"], &["phone"])]);

        assert_eq!(
            ledger.state(0).map(|state| state.is_contributor(&"phone")),
            Some(true)
        );
        assert_eq!(
            ledger.state(1).map(|state| state.is_contributor(&"phone")),
            Some(false)
        );
        assert_eq!(ledger.state(2), None);
    }
}
