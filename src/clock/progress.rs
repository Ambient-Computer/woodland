use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::VersionVector;
use crate::actor::Peer;
use crate::lattice::Lattice;

/// What each contributor says it has integrated.
///
/// Each entry is written only by the contributor it describes, and only ever
/// grows, so the whole map is a lattice: merge entry by entry, keeping the
/// larger vector. That's what makes it something an agreement can agree on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Progress(BTreeMap<Peer, VersionVector>);

impl Progress {
    /// A contributor reports how far it has gotten.
    pub(crate) fn report(contributor: Peer, integrated: VersionVector) -> Self {
        Self(BTreeMap::from([(contributor, integrated)]))
    }

    pub(crate) fn of(&self, contributor: Peer) -> Option<&VersionVector> {
        self.0.get(&contributor)
    }

    /// The greatest lower bound across `contributors`: history that every
    /// one of them has integrated.
    ///
    /// A contributor that hasn't reported yet counts as having nothing, which
    /// pulls the floor to zero. That's deliberate: a new member's arrival
    /// should never let others compact away history it might still need.
    pub(crate) fn floor(&self, contributors: &BTreeSet<Peer>) -> VersionVector {
        let mut reports = contributors.iter().map(|peer| self.of(*peer));

        let Some(Some(first)) = reports.next() else {
            return VersionVector::default();
        };

        reports
            .try_fold(first.clone(), |floor, report| {
                report.map(|report| floor.meet(report))
            })
            .unwrap_or_default()
    }

    /// What a newcomer must hold before it writes: the greatest lower bound
    /// across the `contributors` that have reported, other than itself.
    ///
    /// Unlike the [floor], silent contributors are skipped, not counted
    /// as zero. Skipping one can only raise the bar. If no other
    /// contributor has reported, the bar is everything anyone has: the join
    /// of every report. That covers a configuration whose earlier
    /// contributors were all replaced.
    ///
    /// [floor]: Progress::floor
    pub(crate) fn baseline(&self, contributors: &BTreeSet<Peer>, newcomer: Peer) -> VersionVector {
        let others = contributors.iter().filter(|&&peer| peer != newcomer);
        let mut reports = others.filter_map(|peer| self.of(*peer));

        let Some(first) = reports.next() else {
            let mut everything = VersionVector::default();
            for report in self.0.values() {
                everything.join(report);
            }

            return everything;
        };

        reports.fold(first.clone(), |baseline, report| baseline.meet(report))
    }
}

impl Lattice for Progress {
    fn join(&mut self, other: &Self) -> bool {
        self.0.join(&other.0)
    }

    fn leq(&self, other: &Self) -> bool {
        self.0.leq(&other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::peers::{friend, laptop, nas, phone};

    fn clock(entries: &[(Peer, u64)]) -> VersionVector {
        entries.iter().copied().collect()
    }

    fn progress() -> Progress {
        let mut progress = Progress::report(phone(), clock(&[(phone(), 5), (laptop(), 2)]));
        progress.join(&Progress::report(
            laptop(),
            clock(&[(phone(), 3), (laptop(), 4)]),
        ));

        progress
    }

    #[test]
    fn floor_is_what_everyone_has() {
        let contributors = BTreeSet::from([phone(), laptop()]);

        assert_eq!(
            progress().floor(&contributors),
            clock(&[(phone(), 3), (laptop(), 2)]),
        );
    }

    #[test]
    fn silent_contributors_hold_the_floor_at_zero() {
        let contributors = BTreeSet::from([phone(), laptop(), friend()]);

        assert_eq!(progress().floor(&contributors), VersionVector::default());
    }

    #[test]
    fn non_contributors_dont_count() {
        let mut progress = progress();
        progress.join(&Progress::report(nas(), VersionVector::default()));

        let contributors = BTreeSet::from([phone(), laptop()]);
        assert_eq!(
            progress.floor(&contributors),
            clock(&[(phone(), 3), (laptop(), 2)]),
        );
    }

    #[test]
    fn a_newcomer_starts_from_what_the_others_have() {
        let contributors = BTreeSet::from([phone(), laptop(), friend()]);

        assert_eq!(
            progress().baseline(&contributors, friend()),
            clock(&[(phone(), 3), (laptop(), 2)]),
        );
    }

    #[test]
    fn a_newcomers_own_report_doesnt_lower_its_baseline() {
        let mut progress = progress();
        progress.join(&Progress::report(friend(), VersionVector::default()));
        let contributors = BTreeSet::from([phone(), laptop(), friend()]);

        assert_eq!(
            progress.baseline(&contributors, friend()),
            clock(&[(phone(), 3), (laptop(), 2)]),
        );
    }

    #[test]
    fn a_newcomer_replacing_everyone_starts_from_everything() {
        let contributors = BTreeSet::from([friend()]);

        assert_eq!(
            progress().baseline(&contributors, friend()),
            clock(&[(phone(), 5), (laptop(), 4)]),
        );
    }

    #[test]
    fn reports_only_grow() {
        let mut progress = progress();
        let stale = Progress::report(phone(), clock(&[(phone(), 1)]));

        assert!(!progress.join(&stale));
        assert_eq!(
            progress.of(phone()),
            Some(&clock(&[(phone(), 5), (laptop(), 2)]))
        );
    }
}
