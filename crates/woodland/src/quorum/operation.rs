use std::collections::BTreeSet;

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
        let mut hasher = blake3::Hasher::new();
        update_key(&mut hasher, &self.controller);
        update_keys(&mut hasher, &self.contributors);
        hasher.finalize()
    }
}

impl<PublicKey: AsRef<[u8]>> Change<PublicKey> {
    #[must_use]
    pub fn hash(&self) -> blake3::Hash {
        let mut hasher = blake3::Hasher::new();
        match self {
            Self::SetContributors { contributors } => {
                hasher.update(&[0]);
                update_keys(&mut hasher, contributors);
            }
        }
        hasher.finalize()
    }
}

// Lengths are prefixed so adjacent keys can't be re-split into others.
fn update_key(hasher: &mut blake3::Hasher, key: &impl AsRef<[u8]>) {
    let bytes = key.as_ref();
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn update_keys<PublicKey: AsRef<[u8]>>(hasher: &mut blake3::Hasher, keys: &BTreeSet<PublicKey>) {
    hasher.update(&(keys.len() as u64).to_le_bytes());
    for key in keys {
        update_key(hasher, key);
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
            initialize("controller", &["a", "b"]).id().to_hex().as_str(),
            "e9180a1987483bb56f149bcbdf0dc0cb7d701bd8b7a453c4ed65e12e95b17391",
        );
    }

    #[test]
    fn id_depends_on_every_field() {
        let id = initialize("c", &["a", "b"]).id();

        assert_ne!(id, initialize("d", &["a", "b"]).id());
        assert_ne!(id, initialize("c", &["a"]).id());
    }

    #[test]
    fn key_boundaries_are_unambiguous() {
        assert_ne!(
            set_contributors(&["ab", "c"]).hash(),
            set_contributors(&["a", "bc"]).hash(),
        );
    }
}
