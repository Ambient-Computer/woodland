use std::collections::BTreeSet;

/// Contributors to add and remove, relative to one epoch's contributors.
///
/// Not checked on construction. Everyone handling a change, including its
/// author, must [`validate`](Self::validate) it against the epoch it's for
/// before acting on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContributorChange<PublicKey> {
    pub added: BTreeSet<PublicKey>,
    pub removed: BTreeSet<PublicKey>,
}

/// Why a change doesn't fit an epoch's contributors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("added key is already a contributor")]
    AlreadyContributor,

    #[error("removed key is not a contributor")]
    NotContributor,

    #[error("change would leave the quorum without contributors")]
    NoContributors,
}

impl<PublicKey: Ord + Clone> ContributorChange<PublicKey> {
    /// Checks that this change makes sense for `contributors`.
    ///
    /// # Errors
    ///
    /// Fails if it adds an existing contributor, removes a key that isn't
    /// one, or removes every contributor.
    pub fn validate(&self, contributors: &BTreeSet<PublicKey>) -> Result<(), Error> {
        if !self.added.is_disjoint(contributors) {
            return Err(Error::AlreadyContributor);
        }

        if !self.removed.is_subset(contributors) {
            return Err(Error::NotContributor);
        }

        if self.apply(contributors).is_empty() {
            return Err(Error::NoContributors);
        }

        Ok(())
    }

    /// The contributor set after this change. Assumes the change was
    /// validated against `contributors`.
    #[must_use]
    pub fn apply(&self, contributors: &BTreeSet<PublicKey>) -> BTreeSet<PublicKey> {
        &(contributors | &self.added) - &self.removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Actor, PublicKey, keys, laptop, nas, phone};

    fn change(added: &[&Actor], removed: &[&Actor]) -> ContributorChange<PublicKey> {
        ContributorChange {
            added: keys(added),
            removed: keys(removed),
        }
    }

    #[test]
    fn applies_additions_and_removals() {
        assert_eq!(
            change(&[&nas()], &[&phone()]).apply(&keys(&[&phone(), &laptop()])),
            keys(&[&laptop(), &nas()])
        );
    }

    #[test]
    fn accepts_a_change_that_fits() {
        let contributors = keys(&[&phone(), &laptop()]);

        assert_eq!(
            change(&[&nas()], &[&phone()]).validate(&contributors),
            Ok(())
        );
    }

    #[test]
    fn rejects_adding_a_contributor() {
        assert_eq!(
            change(&[&phone()], &[]).validate(&keys(&[&phone()])),
            Err(Error::AlreadyContributor)
        );
    }

    #[test]
    fn rejects_removing_a_stranger() {
        assert_eq!(
            change(&[], &[&nas()]).validate(&keys(&[&phone()])),
            Err(Error::NotContributor)
        );
    }

    #[test]
    fn rejects_adding_and_removing_the_same_key() {
        assert_eq!(
            change(&[&nas()], &[&nas()]).validate(&keys(&[&phone()])),
            Err(Error::NotContributor)
        );
        assert_eq!(
            change(&[&nas()], &[&nas()]).validate(&keys(&[&phone(), &nas()])),
            Err(Error::AlreadyContributor)
        );
    }

    #[test]
    fn rejects_removing_everyone() {
        assert_eq!(
            change(&[], &[&phone(), &laptop()]).validate(&keys(&[&phone(), &laptop()])),
            Err(Error::NoContributors)
        );
    }
}
