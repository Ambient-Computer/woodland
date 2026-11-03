//! Join-semilattices: values that only grow, and merge by taking the least
//! upper bound.
//!
//! Lattice Agreement is defined over any lattice, and so are state-based
//! CRDTs. Both the agreement's state and the clocks are built from these
//! pieces, so merging is always "join everything you hear", in any order,
//! any number of times.

use std::collections::{BTreeMap, BTreeSet};

pub(crate) trait Lattice {
    /// Merges `other` into `self` (least upper bound). Returns whether
    /// `self` grew, which is how protocols notice new information.
    fn join(&mut self, other: &Self) -> bool;

    /// The lattice order: `self ⊑ other`.
    fn leq(&self, other: &Self) -> bool;
}

/// A grow-only set. Join is union.
impl<T: Ord + Clone> Lattice for BTreeSet<T> {
    fn join(&mut self, other: &Self) -> bool {
        let before = self.len();
        self.extend(other.iter().cloned());

        self.len() != before
    }

    fn leq(&self, other: &Self) -> bool {
        self.is_subset(other)
    }
}

/// A map of lattices. Missing keys are bottom, and join is per key.
impl<K: Ord + Clone, V: Lattice + Clone> Lattice for BTreeMap<K, V> {
    fn join(&mut self, other: &Self) -> bool {
        let mut grew = false;
        for (key, theirs) in other {
            if let Some(ours) = self.get_mut(key) {
                grew |= ours.join(theirs);
            } else {
                self.insert(key.clone(), theirs.clone());
                grew = true;
            }
        }

        grew
    }

    fn leq(&self, other: &Self) -> bool {
        self.iter()
            .all(|(key, ours)| other.get(key).is_some_and(|theirs| ours.leq(theirs)))
    }
}

/// Counters that only grow. Join is max.
impl Lattice for u64 {
    fn join(&mut self, other: &Self) -> bool {
        let grew = *other > *self;
        *self = (*self).max(*other);

        grew
    }

    fn leq(&self, other: &Self) -> bool {
        self <= other
    }
}
