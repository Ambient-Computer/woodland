use std::collections::BTreeSet;

use super::{Change, Initialize};

/// A quorum's state, derived from `Initialize` and the changes decided
/// since.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State<PublicKey> {
    id: blake3::Hash,
    controller: PublicKey,
    contributors: BTreeSet<PublicKey>,
}

impl<PublicKey: Ord + AsRef<[u8]>> State<PublicKey> {
    /// Replays decided changes on top of `Initialize`.
    pub fn derive(
        initialize: Initialize<PublicKey>,
        changes: impl IntoIterator<Item = Change<PublicKey>>,
    ) -> Self {
        let mut state = Self {
            id: initialize.id(),
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

    #[must_use]
    pub fn id(&self) -> blake3::Hash {
        self.id
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

    fn initialize(
        controller: &'static str,
        contributors: &[&'static str],
    ) -> Initialize<&'static str> {
        Initialize {
            controller,
            contributors: contributors.iter().copied().collect(),
        }
    }

    fn set_contributors(contributors: &[&'static str]) -> Change<&'static str> {
        Change::SetContributors {
            contributors: contributors.iter().copied().collect(),
        }
    }

    #[test]
    fn derives_from_initialize() {
        let state = State::derive(initialize("c", &["a", "b"]), []);

        assert_eq!(state.controller(), &"c");
        assert_eq!(state.contributors(), &BTreeSet::from(["a", "b"]));
    }

    #[test]
    fn applies_changes_in_order() {
        let state = State::derive(
            initialize("c", &["a"]),
            [set_contributors(&["a", "b"]), set_contributors(&["b", "d"])],
        );

        assert_eq!(state.contributors(), &BTreeSet::from(["b", "d"]));
    }

    #[test]
    fn majority_is_strict() {
        let even = State::derive(initialize("c", &["1", "2", "3", "4"]), []);
        assert!(!even.is_majority(2));
        assert!(even.is_majority(3));

        let odd = State::derive(initialize("c", &["1", "2", "3"]), []);
        assert!(!odd.is_majority(1));
        assert!(odd.is_majority(2));
    }
}
