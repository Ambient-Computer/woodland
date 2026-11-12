use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::actor::Peer;
use crate::lattice::Lattice;

/// Who the contributors are, as a lattice.
///
/// This is RLA's central trick. A configuration isn't a list that gets
/// replaced; it's the set of every membership change ever proposed. Joining
/// two configurations unions their changes, so concurrent reconfigurations
/// merge instead of conflicting, exactly like the values being agreed on.
///
/// It's a two-phase set: a peer is a member once added and until removed.
/// Removal is permanent. Re-adding a removed device needs a new key.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct Configuration {
    added: BTreeSet<Peer>,
    removed: BTreeSet<Peer>,
}

/// A single membership change, applied on top of a configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Add(Peer),
    Remove(Peer),
}

impl Configuration {
    pub(crate) fn founded_by(founders: impl IntoIterator<Item = Peer>) -> Self {
        Self {
            added: founders.into_iter().collect(),
            removed: BTreeSet::new(),
        }
    }

    /// This configuration plus one change.
    pub(crate) fn with(&self, change: Change) -> Self {
        let mut next = self.clone();
        match change {
            Change::Add(peer) => next.added.insert(peer),
            Change::Remove(peer) => next.removed.insert(peer),
        };

        next
    }

    /// Current contributors: added and not removed.
    pub(crate) fn members(&self) -> BTreeSet<Peer> {
        self.added.difference(&self.removed).copied().collect()
    }

    pub(crate) fn is_member(&self, peer: Peer) -> bool {
        self.added.contains(&peer) && !self.removed.contains(&peer)
    }

    /// Everyone who was a contributor and no longer is.
    pub(crate) fn removed(&self) -> impl Iterator<Item = Peer> + '_ {
        self.added.intersection(&self.removed).copied()
    }

    /// Whether `peer` was ever a contributor, even if removed since.
    pub(crate) fn was_ever_member(&self, peer: Peer) -> bool {
        self.added.contains(&peer)
    }

    /// Whether `holders` include a strict majority of the members.
    ///
    /// Any two majorities of the same configuration share a member. That
    /// overlap is what lattice agreement's safety rests on. A configuration
    /// with no members has no quorum at all, so it can never learn anything.
    pub(crate) fn has_quorum(&self, holders: &BTreeSet<Peer>) -> bool {
        let members = self.members();
        let present = members.intersection(holders).count();

        !members.is_empty() && present * 2 > members.len()
    }
}

impl Lattice for Configuration {
    fn join(&mut self, other: &Self) -> bool {
        let added = self.added.join(&other.added);
        let removed = self.removed.join(&other.removed);

        added || removed
    }

    fn leq(&self, other: &Self) -> bool {
        self.added.leq(&other.added) && self.removed.leq(&other.removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::peers::{friend, laptop, nas, phone};

    fn household() -> Configuration {
        Configuration::founded_by([phone(), laptop(), nas()])
    }

    #[test]
    fn concurrent_changes_merge() {
        let mut with_friend = household().with(Change::Add(friend()));
        let without_nas = household().with(Change::Remove(nas()));
        with_friend.join(&without_nas);

        assert_eq!(
            with_friend.members(),
            BTreeSet::from([phone(), laptop(), friend()])
        );
    }

    #[test]
    fn removal_is_permanent() {
        let gone = household()
            .with(Change::Remove(nas()))
            .with(Change::Add(nas()));

        assert!(!gone.is_member(nas()));
        assert!(gone.was_ever_member(nas()));
    }

    #[test]
    fn quorum_is_a_strict_majority() {
        let config = household().with(Change::Add(friend()));

        assert!(!config.has_quorum(&BTreeSet::from([phone(), laptop()])));
        assert!(config.has_quorum(&BTreeSet::from([phone(), laptop(), nas()])));
    }

    #[test]
    fn removed_members_dont_count_toward_quorum() {
        let config = household().with(Change::Remove(nas()));

        assert!(!config.has_quorum(&BTreeSet::from([phone(), nas()])));
        assert!(config.has_quorum(&BTreeSet::from([phone(), laptop()])));
    }

    #[test]
    fn empty_configurations_never_have_quorum() {
        let config = Configuration::founded_by([phone()]).with(Change::Remove(phone()));

        assert!(!config.has_quorum(&BTreeSet::from([phone()])));
    }
}
