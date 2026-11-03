use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::clock::Stamp;

/// A key-value map where each key is a last-writer-wins register.
///
/// Removal leaves a tombstone: an entry with no value, but with the stamp
/// of the removal. Tombstones are what make a late, older `Set` lose to a
/// newer `Remove` it never saw. Dropping them safely needs a guarantee that
/// no such late write can still arrive, which is a stronger property than
/// the compaction floor gives on its own (see `group::compaction`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KvModel {
    entries: BTreeMap<Vec<u8>, Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Operation {
    Set { key: Vec<u8>, value: Vec<u8> },
    Remove { key: Vec<u8> },
}

impl Operation {
    pub(crate) fn key(&self) -> &[u8] {
        match self {
            Self::Set { key, .. } | Self::Remove { key } => key,
        }
    }
}

/// The current state of one key: its value (or none, if removed), and the
/// stamp of the write that put it there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub(crate) value: Option<Vec<u8>>,
    pub(crate) stamp: Stamp,
}

impl KvModel {
    /// The current value of `key`. Applications read from storage instead
    /// (see [`Query`](super::Query)).
    #[cfg(test)]
    pub(crate) fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.entries.get(key)?.value.as_deref()
    }

    /// Applies one transaction's operations, all under its stamp. Returns
    /// the keys that changed, so the caller can persist just those.
    ///
    /// A write wins if its stamp is at least the current one. "At least",
    /// not "greater", so later operations in the same transaction overwrite
    /// earlier ones. Stamps are unique per transaction, so equality only
    /// happens within one.
    pub(crate) fn apply(&mut self, stamp: Stamp, operations: &[Operation]) -> Vec<Vec<u8>> {
        let mut changed = Vec::new();
        for operation in operations {
            let (key, value) = match operation {
                Operation::Set { key, value } => (key, Some(value.clone())),
                Operation::Remove { key } => (key, None),
            };

            if self.write(key, Entry { value, stamp }) {
                changed.push(key.clone());
            }
        }

        changed
    }

    /// Merges another replica's state, key by key. This is how a device
    /// that fell behind the compaction floor catches up: it can't be sent
    /// the transactions, so it gets their result.
    pub(crate) fn merge(&mut self, other: &KvModel) -> Vec<Vec<u8>> {
        let mut changed = Vec::new();
        for (key, entry) in &other.entries {
            if self.write(key, entry.clone()) {
                changed.push(key.clone());
            }
        }

        changed
    }

    pub(crate) fn entry(&self, key: &[u8]) -> Option<&Entry> {
        self.entries.get(key)
    }

    /// Every key with an entry, tombstones included.
    pub(crate) fn keys(&self) -> impl Iterator<Item = &[u8]> {
        self.entries.keys().map(Vec::as_slice)
    }

    /// Restores one entry from storage.
    pub(crate) fn restore(&mut self, key: Vec<u8>, entry: Entry) {
        self.entries.insert(key, entry);
    }

    fn write(&mut self, key: &[u8], entry: Entry) -> bool {
        let wins = self
            .entries
            .get(key)
            .is_none_or(|current| entry.stamp >= current.stamp);

        if wins {
            self.entries.insert(key.to_vec(), entry);
        }

        wins
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::actor::Peer;
    use crate::testing::peers::{laptop, nas, phone};

    fn stamp(lamport: u64, author: Peer) -> Stamp {
        Stamp { lamport, author }
    }

    fn set(key: &str, value: &str) -> Operation {
        Operation::Set {
            key: key.into(),
            value: value.into(),
        }
    }

    fn remove(key: &str) -> Operation {
        Operation::Remove { key: key.into() }
    }

    #[test]
    fn later_writes_win() {
        let mut model = KvModel::default();
        model.apply(stamp(2, phone()), &[set("color", "blue")]);
        model.apply(stamp(1, laptop()), &[set("color", "red")]);

        assert_eq!(model.get(b"color"), Some(&b"blue"[..]));
    }

    #[test]
    fn concurrent_writes_break_ties_by_author() {
        let (low, high) = if phone() < laptop() {
            (phone(), laptop())
        } else {
            (laptop(), phone())
        };

        let mut model = KvModel::default();
        model.apply(stamp(1, high), &[set("color", "high")]);
        model.apply(stamp(1, low), &[set("color", "low")]);

        assert_eq!(model.get(b"color"), Some(&b"high"[..]));
    }

    #[test]
    fn tombstones_beat_older_writes() {
        let mut model = KvModel::default();
        model.apply(stamp(2, phone()), &[remove("color")]);
        model.apply(stamp(1, laptop()), &[set("color", "red")]);

        assert_eq!(model.get(b"color"), None);
    }

    #[test]
    fn later_operations_in_a_transaction_win() {
        let mut model = KvModel::default();
        model.apply(stamp(1, phone()), &[set("color", "red"), remove("color")]);

        assert_eq!(model.get(b"color"), None);
    }

    fn transaction() -> impl Strategy<Value = (Stamp, Vec<Operation>)> {
        let author = prop_oneof![Just(phone()), Just(laptop()), Just(nas())];
        let key = prop_oneof![Just("a"), Just("b"), Just("c")];
        let operation = prop_oneof![
            (key.clone(), "[xyz]").prop_map(|(key, value)| set(key, &value)),
            key.prop_map(remove),
        ];

        (1..20u64, author, prop::collection::vec(operation, 1..4))
            .prop_map(|(lamport, author, operations)| (stamp(lamport, author), operations))
    }

    /// Real stamps are unique per transaction. Generated ones may collide,
    /// so keep the first of each.
    fn unique(transactions: Vec<(Stamp, Vec<Operation>)>) -> Vec<(Stamp, Vec<Operation>)> {
        let mut unique = BTreeMap::new();
        for (stamp, operations) in transactions {
            unique.entry(stamp).or_insert(operations);
        }

        unique.into_iter().collect()
    }

    fn replay<'a>(transactions: impl Iterator<Item = &'a (Stamp, Vec<Operation>)>) -> KvModel {
        let mut model = KvModel::default();
        for (stamp, operations) in transactions {
            model.apply(*stamp, operations);
        }

        model
    }

    proptest! {
        /// Order of delivery doesn't matter.
        #[test]
        fn converges_in_any_order(
            transactions in prop::collection::vec(transaction(), 1..12),
            rotation in any::<prop::sample::Index>(),
        ) {
            let transactions = unique(transactions);
            let mut shuffled = transactions.clone();
            let by = rotation.index(shuffled.len());
            shuffled.rotate_left(by);
            shuffled.reverse();

            prop_assert_eq!(replay(transactions.iter()), replay(shuffled.iter()));
        }

        /// Merging two replicas' states equals applying everything to one.
        #[test]
        fn merge_matches_replay(
            transactions in prop::collection::vec(transaction(), 0..16),
            split in any::<prop::sample::Index>(),
        ) {
            let transactions = unique(transactions);
            let (left, right) = transactions.split_at(split.index(transactions.len() + 1));

            let mut merged = replay(left.iter());
            merged.merge(&replay(right.iter()));

            prop_assert_eq!(merged, replay(transactions.iter()));
        }
    }
}
