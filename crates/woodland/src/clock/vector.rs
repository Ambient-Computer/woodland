use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::content_hash::{PayloadType, hasher_for, update_key, update_len};
use crate::envelope::Payload;

/// Identifies a contributor's transaction. Both fields only increase, so
/// any gap between two versions is exactly the transactions in between.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// Committed quorum log length when the transaction was created.
    pub epoch: u64,

    /// Transactions created by the contributor.
    pub counter: u64,
}

/// The latest version seen from each contributor.
///
/// Holds exactly the quorum's contributors. A contributor's transactions are
/// applied in order, so a version implies every version before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionVector<PublicKey> {
    versions: BTreeMap<PublicKey, Version>,
}

impl<PublicKey: Ord> VersionVector<PublicKey> {
    /// A vector that has seen nothing from any of `contributors`.
    pub fn new(contributors: impl IntoIterator<Item = PublicKey>) -> Self {
        Self {
            versions: contributors
                .into_iter()
                .map(|contributor| (contributor, Version::default()))
                .collect(),
        }
    }

    pub fn contributors(&self) -> impl Iterator<Item = &PublicKey> {
        self.versions.keys()
    }

    /// Latest version seen from `contributor`, if it is one.
    #[must_use]
    pub fn get(&self, contributor: &PublicKey) -> Option<Version> {
        self.versions.get(contributor).copied()
    }

    /// Creates the next version for `contributor` in `epoch`.
    ///
    /// # Errors
    ///
    /// Fails if `contributor` is not in the vector.
    pub fn increment(&mut self, contributor: &PublicKey, epoch: u64) -> Result<Version, Error> {
        let version = self
            .versions
            .get_mut(contributor)
            .ok_or(Error::UnknownContributor)?;

        *version = Version {
            epoch: epoch.max(version.epoch),
            counter: version.counter + 1,
        };

        Ok(*version)
    }

    /// Moves `contributor` up to `version`. Never moves backwards.
    ///
    /// # Errors
    ///
    /// Fails if `contributor` is not in the vector.
    pub fn advance(&mut self, contributor: &PublicKey, version: Version) -> Result<(), Error> {
        let latest = self
            .versions
            .get_mut(contributor)
            .ok_or(Error::UnknownContributor)?;

        *latest = (*latest).max(version);
        Ok(())
    }

    /// Takes the latest version for every contributor.
    ///
    /// # Errors
    ///
    /// Fails if the vectors have different contributors. Nothing is merged.
    pub fn merge(&mut self, other: &Self) -> Result<(), Error> {
        self.check_contributors(other)?;

        for (ours, theirs) in self.versions.values_mut().zip(other.versions.values()) {
            *ours = (*ours).max(*theirs);
        }

        Ok(())
    }

    /// Causal order. `None` means the vectors are concurrent: each has seen
    /// transactions the other hasn't.
    ///
    /// # Errors
    ///
    /// Fails if the vectors have different contributors.
    pub fn compare(&self, other: &Self) -> Result<Option<Ordering>, Error> {
        self.check_contributors(other)?;

        let mut ordering = Ordering::Equal;
        for (ours, theirs) in self.versions.values().zip(other.versions.values()) {
            match (ordering, ours.cmp(theirs)) {
                (_, Ordering::Equal) => {}
                (Ordering::Equal, next) => ordering = next,
                (current, next) if current != next => return Ok(None),
                _ => {}
            }
        }

        Ok(Some(ordering))
    }

    fn check_contributors(&self, other: &Self) -> Result<(), Error> {
        if self.versions.keys().eq(other.versions.keys()) {
            Ok(())
        } else {
            Err(Error::ContributorMismatch)
        }
    }
}

impl<PublicKey: AsRef<[u8]>> Payload for VersionVector<PublicKey> {
    fn digest(&self) -> blake3::Hash {
        let mut hasher = hasher_for(PayloadType::VersionVector);
        update_len(&mut hasher, self.versions.len());
        for (contributor, version) in &self.versions {
            update_key(&mut hasher, contributor);
            hasher.update(&version.epoch.to_le_bytes());
            hasher.update(&version.counter.to_le_bytes());
        }
        hasher.finalize()
    }
}

impl<PublicKey: Ord> FromIterator<(PublicKey, Version)> for VersionVector<PublicKey> {
    fn from_iter<I: IntoIterator<Item = (PublicKey, Version)>>(versions: I) -> Self {
        Self {
            versions: versions.into_iter().collect(),
        }
    }
}

/// Contributors come from the quorum, so these mean one side is missing a
/// quorum commit, or a bug.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("not a contributor in this vector")]
    UnknownContributor,

    #[error("vectors have different contributors")]
    ContributorMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(epoch: u64, counter: u64) -> Version {
        Version { epoch, counter }
    }

    fn vector(versions: &[(&'static str, Version)]) -> VersionVector<&'static str> {
        versions.iter().copied().collect()
    }

    #[test]
    fn epoch_orders_before_counter() {
        assert!(version(3, 2) > version(2, 10));
    }

    #[test]
    fn starts_with_nothing_seen() {
        let vector = VersionVector::new(["a"]);

        assert_eq!(vector.get(&"a"), Some(Version::default()));
        assert_eq!(vector.get(&"b"), None);
    }

    #[test]
    fn increment_counts_within_an_epoch() -> Result<(), Error> {
        let mut vector = vector(&[("a", version(1, 2))]);

        assert_eq!(vector.increment(&"a", 1)?, version(1, 3));
        Ok(())
    }

    #[test]
    fn increment_keeps_counting_in_a_new_epoch() -> Result<(), Error> {
        let mut vector = vector(&[("a", version(1, 5))]);

        assert_eq!(vector.increment(&"a", 2)?, version(2, 6));
        Ok(())
    }

    #[test]
    fn advance_never_goes_backwards() -> Result<(), Error> {
        let mut vector = vector(&[("a", version(2, 1))]);
        vector.advance(&"a", version(1, 9))?;

        assert_eq!(vector.get(&"a"), Some(version(2, 1)));
        Ok(())
    }

    #[test]
    fn rejects_unknown_contributors() {
        let mut vector = VersionVector::new(["a"]);

        assert_eq!(vector.increment(&"b", 0), Err(Error::UnknownContributor));
        assert_eq!(
            vector.advance(&"b", version(0, 1)),
            Err(Error::UnknownContributor),
        );
    }

    #[test]
    fn merge_takes_the_latest_versions() -> Result<(), Error> {
        let mut ours = vector(&[("a", version(1, 3)), ("b", version(0, 1))]);
        ours.merge(&vector(&[("a", version(0, 9)), ("b", version(1, 1))]))?;

        assert_eq!(ours, vector(&[("a", version(1, 3)), ("b", version(1, 1))]));
        Ok(())
    }

    #[test]
    fn orders_causally() -> Result<(), Error> {
        let before = vector(&[("a", version(0, 1)), ("b", version(0, 0))]);
        let after = vector(&[("a", version(1, 1)), ("b", version(0, 1))]);

        assert_eq!(before.compare(&after)?, Some(Ordering::Less));
        assert_eq!(after.compare(&before)?, Some(Ordering::Greater));
        assert_eq!(after.compare(&after)?, Some(Ordering::Equal));
        Ok(())
    }

    #[test]
    fn detects_concurrency() -> Result<(), Error> {
        let left = vector(&[("a", version(0, 2)), ("b", version(0, 1))]);
        let right = vector(&[("a", version(0, 1)), ("b", version(0, 2))]);

        assert_eq!(left.compare(&right)?, None);
        Ok(())
    }

    #[test]
    fn digest_covers_every_version() {
        let base = vector(&[("a", version(1, 2))]);

        assert_ne!(base.digest(), vector(&[("a", version(2, 2))]).digest());
        assert_ne!(base.digest(), vector(&[("a", version(1, 3))]).digest());
        assert_ne!(base.digest(), vector(&[("b", version(1, 2))]).digest());
    }

    #[test]
    fn rejects_mismatched_contributors() {
        let mut ab = VersionVector::new(["a", "b"]);
        let ac = VersionVector::new(["a", "c"]);

        assert_eq!(ab.compare(&ac), Err(Error::ContributorMismatch));
        assert_eq!(ab.merge(&ac), Err(Error::ContributorMismatch));
    }
}
