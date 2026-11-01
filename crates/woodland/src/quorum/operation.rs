use std::collections::BTreeSet;

use crate::content_hash::{PayloadType, hasher_for, update_key, update_keys};
use crate::envelope::Payload;

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
pub enum Change<PublicKey> {
    /// Replace the set of contributors.
    SetContributors { contributors: BTreeSet<PublicKey> },
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

impl<PublicKey: AsRef<[u8]>> Payload for Change<PublicKey> {
    fn digest(&self) -> blake3::Hash {
        let mut hasher = hasher_for(PayloadType::Change);
        match self {
            // Each variant gets its own number, like payload types do.
            Self::SetContributors { contributors } => {
                hasher.update(&[0]);
                update_keys(&mut hasher, contributors);
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

    fn set_contributors(contributors: &[&'static str]) -> Change<&'static str> {
        Change::SetContributors {
            contributors: contributors.iter().copied().collect(),
        }
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
            set_contributors(&["ab", "c"]).digest(),
            set_contributors(&["a", "bc"]).digest(),
        );
    }
}
