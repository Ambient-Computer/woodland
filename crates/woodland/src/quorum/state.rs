use std::collections::BTreeSet;

use super::{Change, Initialize};

/// A quorum's state, derived from `Initialize` and the changes decided
/// since.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State<PublicKey> {
    controller: PublicKey,
    contributors: BTreeSet<PublicKey>,
}

impl<PublicKey: Ord> State<PublicKey> {
    /// Replays decided changes on top of `Initialize`.
    pub fn derive(
        initialize: Initialize<PublicKey>,
        changes: impl IntoIterator<Item = Change<PublicKey>>,
    ) -> Self {
        let mut state = Self {
            controller: initialize.controller,
            contributors: initialize.contributors,
        };

        for change in changes {
            state.apply(change);
        }

        state
    }

    /// Applies the next decided change.
    pub fn apply(&mut self, change: Change<PublicKey>) {
        match change {
            Change::SetContributors { contributors } => {
                self.contributors = contributors;
            }
        }
    }

    /// Whether `count` contributors are a strict majority.
    #[must_use]
    pub fn is_majority(&self, count: usize) -> bool {
        count > self.contributors.len() / 2
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn initialize(controller: u8, contributors: &[u8]) -> Initialize<u8> {
        Initialize {
            controller,
            contributors: contributors.iter().copied().collect(),
        }
    }

    fn set_contributors(contributors: &[u8]) -> Change<u8> {
        Change::SetContributors {
            contributors: contributors.iter().copied().collect(),
        }
    }

    #[test]
    fn derives_from_initialize() {
        let state = State::derive(initialize(0, &[1, 2]), []);

        assert_eq!(state.controller(), &0);
        assert_eq!(state.contributors(), &BTreeSet::from([1, 2]));
    }

    #[test]
    fn applies_changes_in_order() {
        let state = State::derive(
            initialize(0, &[1]),
            [set_contributors(&[1, 2]), set_contributors(&[2, 3])],
        );

        assert_eq!(state.contributors(), &BTreeSet::from([2, 3]));
    }

    #[test]
    fn majority_is_strict() {
        let even = State::derive(initialize(0, &[1, 2, 3, 4]), []);
        assert!(!even.is_majority(2));
        assert!(even.is_majority(3));

        let odd = State::derive(initialize(0, &[1, 2, 3]), []);
        assert!(!odd.is_majority(1));
        assert!(odd.is_majority(2));
    }
}
