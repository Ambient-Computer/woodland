use std::collections::BTreeSet;

use super::{Change, Initialize};

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
    pub(super) fn merge(&self, commit: &Change<PublicKey>) -> Self {
        match commit {
            Change::SetContributors { contributors } => Self {
                controller: self.controller.clone(),
                contributors: contributors.clone(),
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

    fn state(contributors: &[&'static str]) -> State<&'static str> {
        State::initial(&Initialize {
            controller: "controller",
            contributors: contributors.iter().copied().collect(),
        })
    }

    #[test]
    fn merge_applies_the_commit() {
        let merged = state(&["phone"]).merge(&Change::SetContributors {
            contributors: BTreeSet::from(["laptop", "nas"]),
        });

        assert_eq!(merged.controller(), &"controller");
        assert_eq!(merged.contributors(), &BTreeSet::from(["laptop", "nas"]));
    }

    #[test]
    fn majority_is_strict() {
        let even = state(&["phone", "laptop"]);
        assert!(!even.is_majority(1));
        assert!(even.is_majority(2));

        let odd = state(&["phone", "laptop", "nas"]);
        assert!(!odd.is_majority(1));
        assert!(odd.is_majority(2));
    }
}
