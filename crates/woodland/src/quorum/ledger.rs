use super::{Change, Initialize, State};

/// Every committed quorum operation, in order. The source of truth for
/// quorum state: states are derived from it, never stored on their own.
///
/// The epoch is the number of commits, and indexes the state that was
/// current at the time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ledger<PublicKey> {
    id: blake3::Hash,
    initial_state: Initialize<PublicKey>,
    commits: Vec<Change<PublicKey>>,

    /// One per epoch, so past epochs can be checked without replaying.
    states: Vec<State<PublicKey>>,
}

impl<PublicKey: Ord + Clone + AsRef<[u8]>> Ledger<PublicKey> {
    /// Replays commits on top of the initial state.
    pub fn derive(
        initial_state: Initialize<PublicKey>,
        commits: impl IntoIterator<Item = Change<PublicKey>>,
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

    /// Appends the next commit, starting a new epoch.
    pub fn append(&mut self, commit: Change<PublicKey>) {
        self.states.push(self.current().merge(&commit));
        self.commits.push(commit);
    }
}

impl<PublicKey> Ledger<PublicKey> {
    #[must_use]
    pub fn id(&self) -> blake3::Hash {
        self.id
    }

    #[must_use]
    pub fn initial_state(&self) -> &Initialize<PublicKey> {
        &self.initial_state
    }

    #[must_use]
    pub fn commits(&self) -> &[Change<PublicKey>] {
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
    use std::collections::BTreeSet;

    use super::*;

    fn initialize(contributors: &[&'static str]) -> Initialize<&'static str> {
        Initialize {
            controller: "controller",
            contributors: contributors.iter().copied().collect(),
        }
    }

    fn set_contributors(contributors: &[&'static str]) -> Change<&'static str> {
        Change::SetContributors {
            contributors: contributors.iter().copied().collect(),
        }
    }

    #[test]
    fn starts_at_epoch_zero() {
        let ledger = Ledger::derive(initialize(&["phone", "laptop"]), []);

        assert_eq!(ledger.epoch(), 0);
        assert_eq!(
            ledger.current().contributors(),
            &BTreeSet::from(["phone", "laptop"])
        );
    }

    #[test]
    fn applies_commits_in_order() {
        let ledger = Ledger::derive(
            initialize(&["phone"]),
            [
                set_contributors(&["phone", "laptop"]),
                set_contributors(&["laptop", "nas"]),
            ],
        );

        assert_eq!(ledger.epoch(), 2);
        assert_eq!(
            ledger.current().contributors(),
            &BTreeSet::from(["laptop", "nas"])
        );
    }

    #[test]
    fn keeps_past_states() {
        let ledger = Ledger::derive(initialize(&["phone"]), [set_contributors(&["laptop"])]);

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
