mod contributors;

use std::cmp::Ordering;
use std::collections::BTreeSet;

pub use contributors::{ContributorChange, Error};

use crate::content_hash::{PayloadType, hasher_for, update_key, update_keys};
use crate::envelope::Payload;
use crate::quorum::State;

/// Creates a new quorum. The quorum determines who has write access. All
/// mutable data must be associated with exactly one quorum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Initialize<PublicKey> {
    /// Controller keys are assumed to be globally unique. Quorum identity
    /// comes from this struct, so reusing a key could make two quorums
    /// appear the same. Identity may be used as a cache key. Avoid the
    /// trouble and always generate a fresh key.
    pub controller: PublicKey,

    /// Initial actors with write access. Must not be empty.
    pub contributors: BTreeSet<PublicKey>,
}

/// A change to quorum state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation<PublicKey> {
    /// Add and remove contributors.
    Contributors(ContributorChange<PublicKey>),
}

/// Orders competing proposals, so every contributor picks the same one
/// without coordinating.
///
/// Operations of different kinds are ordered by precedence, listed
/// explicitly so a new kind has to choose its place. Operations of the same
/// kind are ordered by digest: arbitrary, but the same for everyone.
///
/// Proposals could converge better through a CRDT, merging everyone's
/// intent instead of picking one. That matters most offline, where
/// membership changes may be staged and revised long before a vote.
impl<PublicKey: AsRef<[u8]> + Eq> Ord for Operation<PublicKey> {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Contributors(_), Self::Contributors(_)) => {
                self.digest().as_bytes().cmp(other.digest().as_bytes())
            }
        }
    }
}

impl<PublicKey: AsRef<[u8]> + Eq> PartialOrd for Operation<PublicKey> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<PublicKey: Ord + Clone> Operation<PublicKey> {
    /// Checks that this operation makes sense for `state`, the epoch it's
    /// proposed for.
    ///
    /// # Errors
    ///
    /// Fails if the operation doesn't fit the state.
    pub fn validate(&self, state: &State<PublicKey>) -> Result<(), Error> {
        match self {
            Self::Contributors(change) => change.validate(state.contributors()),
        }
    }
}

impl<PublicKey: AsRef<[u8]>> Initialize<PublicKey> {
    /// Identifies the quorum. Primarily useful as a stable cache key.
    #[must_use]
    pub fn id(&self) -> blake3::Hash {
        let mut hasher = hasher_for(PayloadType::Initialize);
        update_key(&mut hasher, &self.controller);
        update_keys(&mut hasher, &self.contributors);
        hasher.finalize()
    }
}

impl<PublicKey: AsRef<[u8]>> Payload for Initialize<PublicKey> {
    fn digest(&self) -> blake3::Hash {
        self.id()
    }
}

impl<PublicKey: AsRef<[u8]>> Payload for Operation<PublicKey> {
    fn digest(&self) -> blake3::Hash {
        let mut hasher = hasher_for(PayloadType::Operation);
        match self {
            // Each variant gets its own number, like payload types do.
            Self::Contributors(change) => {
                hasher.update(&[0]);
                update_keys(&mut hasher, &change.added);
                update_keys(&mut hasher, &change.removed);
            }
        }
        hasher.finalize()
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

    fn add_contributors(added: &[&'static str]) -> Operation<&'static str> {
        Operation::Contributors(ContributorChange {
            added: added.iter().copied().collect(),
            removed: BTreeSet::new(),
        })
    }

    #[test]
    fn id_is_stable() {
        assert_eq!(
            initialize("controller", &["phone", "laptop"])
                .id()
                .to_hex()
                .as_str(),
            "5ef36b99cd4293f04e77ed47fbb6fd7bc1ba1b71b752fd9173489640cbe45efd",
        );
    }

    #[test]
    fn id_depends_on_every_field() {
        let id = initialize("controller", &["phone", "laptop"]).id();

        assert_ne!(
            id,
            initialize("another controller", &["phone", "laptop"]).id()
        );
        assert_ne!(id, initialize("controller", &["phone", "nas"]).id());
    }

    // Crafted keys, not device names: both sets concatenate to the same bytes.
    #[test]
    fn key_boundaries_are_unambiguous() {
        assert_ne!(
            add_contributors(&["ab", "c"]).digest(),
            add_contributors(&["a", "bc"]).digest(),
        );
    }

    #[test]
    fn orders_by_digest() {
        let laptop = add_contributors(&["laptop"]);
        let nas = add_contributors(&["nas"]);

        assert_eq!(
            laptop.cmp(&nas),
            laptop.digest().as_bytes().cmp(nas.digest().as_bytes())
        );
        assert_eq!(laptop.cmp(&nas), nas.cmp(&laptop).reverse());
    }
}
