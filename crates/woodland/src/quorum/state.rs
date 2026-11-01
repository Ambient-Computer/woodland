use std::collections::BTreeSet;

use super::{Initialize, Operation};

/// The quorum as it stands during one epoch, derived from the [`Ledger`](super::Ledger).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State<PublicKey> {
    controller: PublicKey,
    contributors: BTreeSet<PublicKey>,
}

impl<PublicKey: Ord + Clone> State<PublicKey> {
    /// The quorum at epoch 0.
    pub(super) fn initial(initialize: &Initialize<PublicKey>) -> Self {
        Self {
            controller: initialize.controller.clone(),
            contributors: initialize.contributors.clone(),
        }
    }

    /// Merges a commit into this state, producing the next epoch's state.
    /// Commits were validated before they were committed.
    pub(super) fn merge(&self, commit: &Operation<PublicKey>) -> Self {
        match commit {
            Operation::Contributors(change) => Self {
                controller: self.controller.clone(),
                contributors: change.apply(&self.contributors),
            },
        }
    }

    /// Controllers represent an administrator, someone who owns or manages
    /// the data.
    ///
    /// Operations signed by the controller have higher priority and may
    /// perform dangerous actions.
    #[must_use]
    pub fn controller(&self) -> &PublicKey {
        &self.controller
    }

    /// Actors with write access, identified by their public keys. All
    /// operations are verified by signature. Contributors represent
    /// committers in the quorum and can change over time.
    ///
    /// In P2P settings, contributors are devices: a laptop, phone, or
    /// self-hosted server.
    #[must_use]
    pub fn contributors(&self) -> &BTreeSet<PublicKey> {
        &self.contributors
    }

    #[must_use]
    pub fn is_contributor(&self, key: &PublicKey) -> bool {
        self.contributors.contains(key)
    }

    /// Whether `count` contributors are a strict majority.
    #[must_use]
    pub fn is_majority(&self, count: usize) -> bool {
        count > self.contributors.len() / 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quorum::ContributorChange;
    use crate::testing::{keys, laptop, nas, phone, quorum};

    #[test]
    fn merge_applies_the_commit() {
        let ledger = quorum(&[&phone()]);
        let merged = ledger
            .current()
            .merge(&Operation::Contributors(ContributorChange {
                added: keys(&[&laptop(), &nas()]),
                removed: keys(&[&phone()]),
            }));

        assert_eq!(merged.controller(), &ledger.initial_state().controller);
        assert_eq!(merged.contributors(), &keys(&[&laptop(), &nas()]));
    }

    #[test]
    fn majority_is_strict() {
        let even = quorum(&[&phone(), &laptop()]);
        assert!(!even.current().is_majority(1));
        assert!(even.current().is_majority(2));

        let odd = quorum(&[&phone(), &laptop(), &nas()]);
        assert!(!odd.current().is_majority(1));
        assert!(odd.current().is_majority(2));
    }
}
