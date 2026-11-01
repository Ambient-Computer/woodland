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
    use crate::testing::{controller, keys, laptop, nas, phone, quorum};

    #[test]
    fn id_is_stable() {
        let initialize = Initialize {
            controller: controller().key(),
            contributors: keys(&[&phone(), &laptop()]),
        };

        assert_eq!(
            initialize.id().to_hex().as_str(),
            "54e425629998d30a32f61faa4fc5f55e4a5719f8fee6434e99829bfd2accd5a9"
        );
    }

    #[test]
    fn id_depends_on_every_field() {
        let initialize = quorum(&[&phone(), &laptop()]).initial_state().clone();

        let other_controller = Initialize {
            controller: quorum(&[&phone(), &laptop()]).initial_state().controller,
            ..initialize.clone()
        };
        let other_contributors = Initialize {
            contributors: keys(&[&phone(), &nas()]),
            ..initialize.clone()
        };

        assert_ne!(initialize.id(), other_controller.id());
        assert_ne!(initialize.id(), other_contributors.id());
    }

    // Crafted keys, not actors: both sets concatenate to the same bytes.
    #[test]
    fn key_boundaries_are_unambiguous() {
        let add = |added: &[&'static str]| {
            Operation::Contributors(ContributorChange {
                added: added.iter().copied().collect(),
                removed: BTreeSet::new(),
            })
        };

        assert_ne!(add(&["ab", "c"]).digest(), add(&["a", "bc"]).digest());
    }

    #[test]
    fn orders_by_digest() {
        let add = |added| {
            Operation::Contributors(ContributorChange {
                added,
                removed: BTreeSet::new(),
            })
        };
        let laptop = add(keys(&[&laptop()]));
        let nas = add(keys(&[&nas()]));

        assert_eq!(
            laptop.cmp(&nas),
            laptop.digest().as_bytes().cmp(nas.digest().as_bytes())
        );
        assert_eq!(laptop.cmp(&nas), nas.cmp(&laptop).reverse());
    }
}
