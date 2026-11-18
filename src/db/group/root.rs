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
//!
//! Beside it is a second sum, of the blobs that state refers to. Its
//! name takes no key, so a device that can't read the state can still
//! check a list of them against a checkpoint, and is never told what
//! to let go of by whoever handed it the state.

use serde::{Deserialize, Serialize};

use crate::blob::Listed;
use crate::encoding;
use crate::model::{self, ModelId};
use crate::seal::Keyring;

const LANES: usize = 1024;

/// Names a committed state, and the blobs it refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Root {
    state: [u8; 32],
    /// Each as many times as the state refers to it.
    blobs: [u8; 32],
}

/// The running sums a [`Root`] is the hashes of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Accumulator {
    state: Vec<u16>,
    blobs: Vec<u16>,
}

impl Default for Accumulator {
    fn default() -> Self {
        Self {
            state: vec![0; LANES],
            blobs: vec![0; LANES],
        }
    }
}

fn bytes_of(lanes: &[u16]) -> Vec<u8> {
    lanes.iter().flat_map(|lane| lane.to_le_bytes()).collect()
}

/// What the blobs a state refers to are called, from their sum.
fn named(blobs: &[u16]) -> [u8; 32] {
    encoding::hash("woodland/state/blobs", &bytes_of(blobs))
}

impl Root {
    /// Whether `listed` is every blob the state refers to, each as
    /// many times as it does. It's all a device that can't read the
    /// state can check of it.
    // Nothing takes a list on trust until relays count blobs.
    #[cfg_attr(not(test), expect(dead_code, reason = "used once relays go by lists"))]
    pub(crate) fn lists(&self, listed: &[Listed]) -> bool {
        let mut sum = vec![0; LANES];
        for blob in listed {
            add(&mut sum, &blob_row(*blob));
        }

        named(&sum) == self.blobs
    }
}

impl Accumulator {
    pub(super) fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let (state, blobs) = bytes.split_at_checked(LANES * 2)?;
        let whole = blobs.len() == LANES * 2;

        whole.then(|| Self {
            state: lanes_of(state),
            blobs: lanes_of(blobs),
        })
    }

    pub(super) fn to_bytes(&self) -> Vec<u8> {
        [bytes_of(&self.state), bytes_of(&self.blobs)].concat()
    }

    /// The name of the state this is the sum of, to those who hold
    /// `keys`.
    pub(super) fn root(&self, keys: &Keyring) -> Root {
        Root {
            state: keys.name(&bytes_of(&self.state)),
            blobs: named(&self.blobs),
        }
    }

    /// `key` of `model` holds `value`.
    pub(super) fn hold(&mut self, model: ModelId, key: &[u8], value: &[u8]) {
        add(&mut self.state, &row(b'v', model, key, value));
        if let Some(blob) = model::blob_under(model, key, value) {
            add(&mut self.blobs, &blob_row(blob.listed()));
        }
    }

    /// `key` of `model` no longer holds `value`.
    pub(super) fn clear(&mut self, model: ModelId, key: &[u8], value: &[u8]) {
        subtract(&mut self.state, &row(b'v', model, key, value));
        if let Some(blob) = model::blob_under(model, key, value) {
            subtract(&mut self.blobs, &blob_row(blob.listed()));
        }
    }

    /// `model` is dropped. It stays dropped, so there's no taking it back.
    pub(super) fn drop(&mut self, model: ModelId) {
        add(&mut self.state, &row(b'd', model, b"", b""));
    }
}

fn add(sum: &mut [u16], row: &[u16]) {
    for (lane, add) in sum.iter_mut().zip(row) {
        *lane = lane.wrapping_add(*add);
    }
}

fn subtract(sum: &mut [u16], row: &[u16]) {
    for (lane, subtract) in sum.iter_mut().zip(row) {
        *lane = lane.wrapping_sub(*subtract);
    }
}

/// What one reference to a blob adds to the sum of them.
fn blob_row(blob: Listed) -> Vec<u16> {
    let mut hasher = blake3::Hasher::new_derive_key("woodland/state/blob");
    hasher.update(&blob.name);
    hasher.update(&blob.length.to_le_bytes());

    let mut bytes = vec![0; LANES * 2];
    hasher.finalize_xof().fill(&mut bytes);

    lanes_of(&bytes)
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
    use crate::blob::BlobRef;
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

    /// A row of a table with a picture in it.
    fn pictured(sum: &mut Accumulator, row: &[u8], blob: BlobRef, hold: bool) {
        use crate::model::table::Place;
        use crate::model::{ColumnId, Value};

        let table = ModelId::table(GroupId::from_bytes([1; 32]), [1; 32]);
        let key = Place::Cell(row.to_vec(), [0; 16], ColumnId::blob(table, [2; 32])).key();
        let value = encoding::encode(&Value::Blob(blob));
        if hold {
            sum.hold(table, &key, &value);
        } else {
            sum.clear(table, &key, &value);
        }
    }

    #[test]
    fn a_root_vouches_for_the_blobs_a_state_refers_to() {
        let blob = |name: u8| BlobRef::new([name; 32], 10, None);
        let mut held = Accumulator::default();
        let empty = held.root(&Keyring::Open);
        assert!(empty.lists(&[]));

        pictured(&mut held, b"ada", blob(1), true);
        pictured(&mut held, b"grace", blob(2), true);
        pictured(&mut held, b"alan", blob(2), true);
        let root = held.root(&Keyring::Open);
        let (one, two) = (blob(1).listed(), blob(2).listed());

        assert!(root.lists(&[two, one, two]), "in any order");
        assert!(
            !root.lists(&[one, two]),
            "as many times as it's referred to"
        );
        assert!(!root.lists(&[]));
        let longer = BlobRef::new([1; 32], 11, None).listed();
        assert!(!root.lists(&[longer, two, two]), "a blob is its length too");

        pictured(&mut held, b"ada", blob(1), false);
        pictured(&mut held, b"grace", blob(2), false);
        pictured(&mut held, b"alan", blob(2), false);
        assert_eq!(held.root(&Keyring::Open), empty);
    }

    /// A relay can check a list without the key the state is named
    /// under.
    #[test]
    fn the_blobs_a_sealed_state_refers_to_are_named_without_its_key() {
        let blob = BlobRef::new([1; 32], 10, Some([3; 32]));
        let mut held = Accumulator::default();
        pictured(&mut held, b"ada", blob, true);

        assert!(held.root(&Keyring::sealed()).lists(&[blob.listed()]));
    }

    /// A value that only looks like a reference isn't counted as one.
    #[test]
    fn only_a_cell_of_a_column_of_blobs_is_a_reference() {
        use crate::model::Value;

        let blob = BlobRef::new([1; 32], 10, None);
        let mut held = Accumulator::default();
        held.hold(model(1), b"key", &encoding::encode(&Value::Blob(blob)));

        assert!(held.root(&Keyring::Open).lists(&[]));
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
