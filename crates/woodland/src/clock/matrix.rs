use std::cmp::Ordering;
use std::collections::BTreeMap;

use super::{Error, VersionVector};

/// A matrix clock: every contributor's version vector, as last seen.
///
/// Each row is what its owner has seen. The rows' greatest lower bound is
/// what every contributor has seen, which is safe to checkpoint or compact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matrix<PublicKey> {
    rows: BTreeMap<PublicKey, VersionVector<PublicKey>>,
}

/// How an offered row compares to the one we have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum Freshness {
    /// The row is newer than ours.
    Newer,

    /// We already had this row or a newer one.
    Stale,

    /// The row is concurrent with ours. An owner's view only grows, so the
    /// owner signed two incompatible histories.
    Conflict,
}

impl<PublicKey: Ord + Clone> Matrix<PublicKey> {
    /// A matrix where no contributor has seen anything.
    pub fn new(contributors: impl IntoIterator<Item = PublicKey>) -> Self {
        let contributors: Vec<_> = contributors.into_iter().collect();
        let row = VersionVector::new(contributors.iter().cloned());

        Self {
            rows: contributors
                .into_iter()
                .map(|owner| (owner, row.clone()))
                .collect(),
        }
    }

    /// What `owner` has seen, as last reported.
    #[must_use]
    pub fn row(&self, owner: &PublicKey) -> Option<&VersionVector<PublicKey>> {
        self.rows.get(owner)
    }

    /// Mutable access to `owner`'s row, for maintaining our own. Rows must
    /// only grow.
    pub fn row_mut(&mut self, owner: &PublicKey) -> Option<&mut VersionVector<PublicKey>> {
        self.rows.get_mut(owner)
    }

    /// Compares `row` to `owner`'s current row without applying it, so
    /// only newer rows need their signatures verified.
    ///
    /// # Errors
    ///
    /// Fails if `owner` is not a contributor, or `row` has different
    /// contributors.
    pub fn freshness(
        &self,
        owner: &PublicKey,
        row: &VersionVector<PublicKey>,
    ) -> Result<Freshness, Error> {
        let current = self.rows.get(owner).ok_or(Error::UnknownContributor)?;

        Ok(match row.compare(current)? {
            Some(Ordering::Greater) => Freshness::Newer,
            Some(Ordering::Less | Ordering::Equal) => Freshness::Stale,
            None => Freshness::Conflict,
        })
    }

    /// Replaces `owner`'s row if `row` is newer.
    ///
    /// # Errors
    ///
    /// Fails if `owner` is not a contributor, or `row` has different
    /// contributors.
    pub fn update(
        &mut self,
        owner: &PublicKey,
        row: VersionVector<PublicKey>,
    ) -> Result<Freshness, Error> {
        let freshness = self.freshness(owner, &row)?;
        if freshness == Freshness::Newer {
            self.rows.insert(owner.clone(), row);
        }

        Ok(freshness)
    }

    /// What every contributor has seen.
    #[must_use]
    pub fn lower_bound(&self) -> VersionVector<PublicKey> {
        self.rows
            .keys()
            .map(|contributor| {
                let seen = self
                    .rows
                    .values()
                    .filter_map(|row| row.get(contributor))
                    .min()
                    .unwrap_or_default();

                (contributor.clone(), seen)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Version;

    fn row(versions: &[(&'static str, u64)]) -> VersionVector<&'static str> {
        versions
            .iter()
            .map(|&(contributor, counter)| (contributor, Version { epoch: 0, counter }))
            .collect()
    }

    #[test]
    fn applies_newer_rows() -> Result<(), Error> {
        let mut matrix = Matrix::new(["a", "b"]);

        assert_eq!(
            matrix.update(&"a", row(&[("a", 1), ("b", 0)]))?,
            Freshness::Newer
        );
        assert_eq!(
            matrix.update(&"a", row(&[("a", 2), ("b", 0)]))?,
            Freshness::Newer
        );
        assert_eq!(matrix.row(&"a"), Some(&row(&[("a", 2), ("b", 0)])));
        Ok(())
    }

    #[test]
    fn freshness_does_not_apply() -> Result<(), Error> {
        let matrix = Matrix::new(["a", "b"]);
        let newer = row(&[("a", 1), ("b", 0)]);

        assert_eq!(matrix.freshness(&"a", &newer)?, Freshness::Newer);
        assert_eq!(matrix.row(&"a"), Some(&row(&[("a", 0), ("b", 0)])));
        Ok(())
    }

    #[test]
    fn ignores_stale_rows() -> Result<(), Error> {
        let mut matrix = Matrix::new(["a", "b"]);
        let _ = matrix.update(&"a", row(&[("a", 2), ("b", 0)]))?;

        assert_eq!(
            matrix.update(&"a", row(&[("a", 1), ("b", 0)]))?,
            Freshness::Stale
        );
        assert_eq!(
            matrix.update(&"a", row(&[("a", 2), ("b", 0)]))?,
            Freshness::Stale
        );
        Ok(())
    }

    #[test]
    fn reports_conflicting_rows() -> Result<(), Error> {
        let mut matrix = Matrix::new(["a", "b"]);
        let _ = matrix.update(&"a", row(&[("a", 2), ("b", 1)]))?;

        assert_eq!(
            matrix.update(&"a", row(&[("a", 1), ("b", 2)]))?,
            Freshness::Conflict
        );
        Ok(())
    }

    #[test]
    fn rejects_unknown_contributors() {
        let mut matrix = Matrix::new(["a", "b"]);

        assert_eq!(
            matrix.update(&"c", row(&[("a", 1), ("b", 0)])),
            Err(Error::UnknownContributor),
        );
        assert_eq!(
            matrix.update(&"a", row(&[("a", 1), ("c", 0)])),
            Err(Error::ContributorMismatch),
        );
    }

    #[test]
    fn lower_bound_is_what_everyone_has_seen() -> Result<(), Error> {
        let mut matrix = Matrix::new(["a", "b"]);
        let _ = matrix.update(&"a", row(&[("a", 3), ("b", 1)]))?;
        let _ = matrix.update(&"b", row(&[("a", 2), ("b", 2)]))?;

        assert_eq!(matrix.lower_bound(), row(&[("a", 2), ("b", 1)]));
        Ok(())
    }

    #[test]
    fn lower_bound_waits_for_silent_contributors() -> Result<(), Error> {
        let mut matrix = Matrix::new(["a", "b"]);
        let _ = matrix.update(&"a", row(&[("a", 3), ("b", 1)]))?;

        assert_eq!(matrix.lower_bound(), row(&[("a", 0), ("b", 0)]));
        Ok(())
    }
}
