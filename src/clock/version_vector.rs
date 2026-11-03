use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::actor::Peer;
use crate::lattice::Lattice;

/// Per author, the highest transaction sequence number integrated.
///
/// Sequence numbers start at 1 and are integrated without gaps, so an entry
/// of `n` means "all of 1..=n". Authors with no entry are at 0.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct VersionVector(BTreeMap<Peer, u64>);

impl VersionVector {
    pub(crate) fn get(&self, author: Peer) -> u64 {
        self.0.get(&author).copied().unwrap_or(0)
    }

    /// Whether transaction `seq` from `author` is already integrated.
    pub(crate) fn contains(&self, author: Peer, seq: u64) -> bool {
        seq <= self.get(author)
    }

    /// The sequence number `author` should use next.
    pub(crate) fn next(&self, author: Peer) -> u64 {
        self.get(author) + 1
    }

    /// Records that `seq` from `author` was integrated. Callers integrate in
    /// order, so this only ever moves forward.
    pub(crate) fn advance(&mut self, author: Peer, seq: u64) {
        let entry = self.0.entry(author).or_default();
        *entry = (*entry).max(seq);
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (Peer, u64)> + '_ {
        self.0.iter().map(|(&author, &seq)| (author, seq))
    }

    /// Greatest lower bound: what both sides have. Authors missing from
    /// either side are at 0, so they drop out.
    pub(crate) fn meet(&self, other: &Self) -> Self {
        let entries = self
            .iter()
            .map(|(author, seq)| (author, seq.min(other.get(author))))
            .filter(|&(_, seq)| seq > 0);

        Self(entries.collect())
    }
}

impl Lattice for VersionVector {
    fn join(&mut self, other: &Self) -> bool {
        self.0.join(&other.0)
    }

    fn leq(&self, other: &Self) -> bool {
        self.iter().all(|(author, seq)| seq <= other.get(author))
    }
}

impl FromIterator<(Peer, u64)> for VersionVector {
    fn from_iter<I: IntoIterator<Item = (Peer, u64)>>(entries: I) -> Self {
        Self(entries.into_iter().filter(|&(_, seq)| seq > 0).collect())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::testing::peers::{laptop, nas, phone};

    fn vector(phone_seq: u64, laptop_seq: u64, nas_seq: u64) -> VersionVector {
        [
            (phone(), phone_seq),
            (laptop(), laptop_seq),
            (nas(), nas_seq),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn missing_authors_are_zero() {
        let clock = vector(2, 0, 0);

        assert_eq!(clock.get(laptop()), 0);
        assert!(clock.contains(phone(), 2));
        assert!(!clock.contains(phone(), 3));
        assert_eq!(clock.next(phone()), 3);
    }

    #[test]
    fn meet_keeps_what_both_have() {
        let meet = vector(5, 1, 0).meet(&vector(3, 4, 2));

        assert_eq!(meet, vector(3, 1, 0));
    }

    proptest! {
        #[test]
        fn meet_is_below_both_and_join_above_both(
            left in (0..5u64, 0..5u64, 0..5u64),
            right in (0..5u64, 0..5u64, 0..5u64),
        ) {
            let left = vector(left.0, left.1, left.2);
            let right = vector(right.0, right.1, right.2);

            let meet = left.meet(&right);
            prop_assert!(meet.leq(&left) && meet.leq(&right));

            let mut join = left.clone();
            join.join(&right);
            prop_assert!(left.leq(&join) && right.leq(&join));
        }
    }
}
