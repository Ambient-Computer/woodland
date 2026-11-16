//! The state root: a short name for everything a group has committed.
//!
//! A checkpoint carries one, so that every device that commits it can
//! tell it reached the same state as the quorum that voted, and so a
//! device handed that state by someone else can tell it's the real one.
//!
//! The root is the hash of a sum. Each committed value, and each dropped
//! model, hashes to a long row of small numbers, and the rows are added
//! up. Adding and subtracting are all that's needed to keep the sum
//! current, so what a checkpoint costs follows what it changed and not
//! what the group holds. The sum doesn't depend on the order things were
//! added in, so two devices that got to the same state by different
//! routes hold the same one.
//!
//! The row has to be long. A sum of short hashes can be forged: given
//! enough candidates, a set that adds up to a chosen total can be found
//! far faster than a single collision. With 1024 numbers of 16 bits each
//! the search is out of reach (this is the `LtHash` construction).
//!
//! What it gives up: a sum can't vouch for part of a state. A copy is
//! checked whole.

use serde::{Deserialize, Serialize};

use crate::model::ModelId;
use crate::seal::Keyring;

const LANES: usize = 1024;

/// Names a committed state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Root([u8; 32]);

/// The running sum a [`Root`] is the hash of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Accumulator(Vec<u16>);

impl Default for Accumulator {
    fn default() -> Self {
        Self(vec![0; LANES])
    }
}

impl Accumulator {
    pub(super) fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let lanes: Vec<u16> = lanes_of(bytes);

        (bytes.len() == LANES * 2).then_some(Self(lanes))
    }

    pub(super) fn to_bytes(&self) -> Vec<u8> {
        self.0.iter().flat_map(|lane| lane.to_le_bytes()).collect()
    }

    /// The name of the state this is the sum of, to those who hold
    /// `keys`.
    pub(super) fn root(&self, keys: &Keyring) -> Root {
        Root(keys.name(&self.to_bytes()))
    }

    /// `key` of `model` holds `value`.
    pub(super) fn hold(&mut self, model: ModelId, key: &[u8], value: &[u8]) {
        self.add(&row(b'v', model, key, value));
    }

    /// `key` of `model` no longer holds `value`.
    pub(super) fn clear(&mut self, model: ModelId, key: &[u8], value: &[u8]) {
        self.subtract(&row(b'v', model, key, value));
    }

    /// `model` is dropped. It stays dropped, so there's no taking it back.
    pub(super) fn drop(&mut self, model: ModelId) {
        self.add(&row(b'd', model, b"", b""));
    }

    fn add(&mut self, row: &[u16]) {
        for (lane, add) in self.0.iter_mut().zip(row) {
            *lane = lane.wrapping_add(*add);
        }
    }

    fn subtract(&mut self, row: &[u16]) {
        for (lane, subtract) in self.0.iter_mut().zip(row) {
            *lane = lane.wrapping_sub(*subtract);
        }
    }
}

/// What one thing adds to the sum. Lengths are hashed with the bytes, so
/// no two things share a row by sliding a byte from key to value.
fn row(kind: u8, model: ModelId, key: &[u8], value: &[u8]) -> Vec<u16> {
    let length = |bytes: &[u8]| u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes();
    let mut hasher = blake3::Hasher::new_derive_key("woodland/state/row");
    hasher.update(&[kind]);
    hasher.update(&model.as_bytes());
    hasher.update(&length(key));
    hasher.update(key);
    hasher.update(&length(value));
    hasher.update(value);

    let mut bytes = vec![0; LANES * 2];
    hasher.finalize_xof().fill(&mut bytes);

    lanes_of(&bytes)
}

fn lanes_of(bytes: &[u8]) -> Vec<u16> {
    let (lanes, _) = bytes.as_chunks::<2>();

    lanes.iter().map(|lane| u16::from_le_bytes(*lane)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::GroupId;

    fn model(nonce: u8) -> ModelId {
        ModelId::kv(GroupId::from_bytes([1; 32]), [nonce; 32])
    }

    #[test]
    fn the_order_things_happened_in_doesnt_matter() {
        let mut one = Accumulator::default();
        one.hold(model(1), b"a", b"1");
        one.hold(model(1), b"b", b"2");
        one.drop(model(2));
        let mut other = Accumulator::default();
        other.drop(model(2));
        other.hold(model(1), b"b", b"2");
        other.hold(model(1), b"a", b"1");

        assert_eq!(one.root(&Keyring::Open), other.root(&Keyring::Open));
    }

    #[test]
    fn clearing_a_value_is_as_if_it_was_never_held() {
        let mut held = Accumulator::default();
        held.hold(model(1), b"a", b"1");
        let with_one = held.root(&Keyring::Open);
        held.hold(model(1), b"b", b"2");
        assert_ne!(held.root(&Keyring::Open), with_one);

        held.clear(model(1), b"b", b"2");

        assert_eq!(held.root(&Keyring::Open), with_one);
    }

    #[test]
    fn different_states_have_different_roots() {
        let root = |model: ModelId, key: &[u8], value: &[u8]| {
            let mut held = Accumulator::default();
            held.hold(model, key, value);

            held.root(&Keyring::Open)
        };
        let roots = [
            root(model(1), b"a", b"1"),
            root(model(2), b"a", b"1"),
            root(model(1), b"a1", b""),
            root(model(1), b"", b"a1"),
            Accumulator::default().root(&Keyring::Open),
        ];

        for (index, root) in roots.iter().enumerate() {
            assert!(!roots[index + 1..].contains(root));
        }
    }

    #[test]
    fn a_sum_survives_storage() {
        let mut held = Accumulator::default();
        held.hold(model(1), b"a", b"1");

        assert_eq!(Accumulator::from_bytes(&held.to_bytes()), Some(held));
        assert_eq!(Accumulator::from_bytes(b"short"), None);
    }

    /// A relay that guesses a sealed group's whole state still can't
    /// tell from a checkpoint whether it guessed right.
    #[test]
    fn a_sealed_groups_root_says_nothing_without_its_key() {
        use crate::actor::Identity;
        use crate::seal::Secret;

        let identity = Identity::from_secret_key([1; 32]);
        let keys = |purpose: &[u8]| {
            let mut keys = Keyring::sealed();
            keys.hold_blind(Secret::make(&identity, purpose));

            keys
        };
        let mut held = Accumulator::default();
        held.hold(model(1), b"pin", b"1234");

        assert_ne!(held.root(&keys(b"ours")), held.root(&Keyring::Open));
        assert_ne!(held.root(&keys(b"ours")), held.root(&keys(b"theirs")));
        assert_ne!(held.root(&keys(b"ours")), held.root(&Keyring::sealed()));
        assert_eq!(held.root(&keys(b"ours")), held.root(&keys(b"ours")));
    }
}
