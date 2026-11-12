//! A key-value map, derived from the timeline: each key holds whatever the
//! last transaction to write it left there.
//!
//! # Versions
//!
//! Every write is kept as a [`Version`] of its key, under its transaction's
//! [`Stamp`]. Stamps are the timeline's order, so a key's versions, sorted
//! by stamp, are that key's history, and the last one is its current
//! value.
//!
//! That makes writing blind. A transaction's versions go straight to
//! storage, wherever they land in the timeline, without reading what's
//! already there. A write that arrives late, with an older stamp, slots in
//! behind the newer ones and changes nothing a reader sees. The comparison
//! that last-writer-wins needs still happens, but in the order of storage
//! keys instead of in memory (see `storage::keys`).
//!
//! Versions are a grow-only set, so replicas merge by union: applying the
//! same transaction twice, or taking a snapshot that overlaps what's
//! already here, writes the same versions again.
//!
//! # Removal
//!
//! Removing a key writes a version with no value: a tombstone. It's what
//! makes a late, older `Set` lose to a newer `Remove` it never saw.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::clock::Stamp;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Operation {
    Set { key: Vec<u8>, value: Vec<u8> },
    Remove { key: Vec<u8> },
}

/// What one transaction left a key holding: a value, or nothing if it
/// removed the key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Version(pub(crate) Option<Vec<u8>>);

impl Operation {
    pub(crate) fn key(&self) -> &[u8] {
        match self {
            Self::Set { key, .. } | Self::Remove { key } => key,
        }
    }

    /// What the key holds once this operation is applied.
    pub(crate) fn version(&self) -> Version {
        match self {
            Self::Set { value, .. } => Version(Some(value.clone())),
            Self::Remove { .. } => Version(None),
        }
    }
}

/// The current value of every key that has one: its last version in the
/// timeline, unless that's a tombstone.
pub(crate) fn current(
    versions: impl IntoIterator<Item = (Vec<u8>, Stamp, Version)>,
) -> BTreeMap<Vec<u8>, Vec<u8>> {
    let mut latest: BTreeMap<Vec<u8>, (Stamp, Version)> = BTreeMap::new();
    for (key, stamp, version) in versions {
        let newer = latest.get(&key).is_none_or(|(held, _)| *held <= stamp);
        if newer {
            latest.insert(key, (stamp, version));
        }
    }

    let values = latest
        .into_iter()
        .filter_map(|(key, (_, Version(value)))| Some((key, value?)));

    values.collect()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::actor::Peer;
    use crate::testing::peers::{laptop, nas, phone};

    /// Plays storage: every version, by key and stamp. Like storage, a
    /// later write to the same key and stamp replaces the earlier one.
    type Versions = BTreeMap<(Vec<u8>, Stamp), Version>;

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

    /// Applies one transaction's operations, all under its stamp.
    fn apply(versions: &mut Versions, stamp: Stamp, operations: &[Operation]) {
        for operation in operations {
            versions.insert((operation.key().to_vec(), stamp), operation.version());
        }
    }

    fn read(versions: &Versions) -> BTreeMap<Vec<u8>, Vec<u8>> {
        let versions = versions.clone().into_iter();

        current(versions.map(|((key, stamp), version)| (key, stamp, version)))
    }

    fn get(versions: &Versions, key: &str) -> Option<String> {
        let value = read(versions).remove(key.as_bytes())?;

        Some(String::from_utf8_lossy(&value).into_owned())
    }

    #[test]
    fn later_writes_win() {
        let mut versions = Versions::new();
        apply(&mut versions, stamp(2, phone()), &[set("color", "blue")]);
        apply(&mut versions, stamp(1, laptop()), &[set("color", "red")]);

        assert_eq!(get(&versions, "color").as_deref(), Some("blue"));
    }

    #[test]
    fn concurrent_writes_break_ties_by_author() {
        let (low, high) = if phone() < laptop() {
            (phone(), laptop())
        } else {
            (laptop(), phone())
        };

        let mut versions = Versions::new();
        apply(&mut versions, stamp(1, high), &[set("color", "high")]);
        apply(&mut versions, stamp(1, low), &[set("color", "low")]);

        assert_eq!(get(&versions, "color").as_deref(), Some("high"));
    }

    #[test]
    fn tombstones_beat_older_writes() {
        let mut versions = Versions::new();
        apply(&mut versions, stamp(2, phone()), &[remove("color")]);
        apply(&mut versions, stamp(1, laptop()), &[set("color", "red")]);

        assert_eq!(get(&versions, "color"), None);
    }

    #[test]
    fn later_operations_in_a_transaction_win() {
        let mut versions = Versions::new();
        let operations = [set("color", "red"), remove("color")];
        apply(&mut versions, stamp(1, phone()), &operations);

        assert_eq!(get(&versions, "color"), None);
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

    fn replay<'a>(transactions: impl Iterator<Item = &'a (Stamp, Vec<Operation>)>) -> Versions {
        let mut versions = Versions::new();
        for (stamp, operations) in transactions {
            apply(&mut versions, *stamp, operations);
        }

        versions
    }

    /// What a reader sees is what applying the transactions one at a time,
    /// in timeline order, would leave behind.
    fn in_timeline_order(transactions: &[(Stamp, Vec<Operation>)]) -> BTreeMap<Vec<u8>, Vec<u8>> {
        let mut state = BTreeMap::new();
        for (_, operations) in transactions {
            for operation in operations {
                match operation.version() {
                    Version(Some(value)) => state.insert(operation.key().to_vec(), value),
                    Version(None) => state.remove(operation.key()),
                };
            }
        }

        state
    }

    proptest! {
        /// Order of delivery doesn't matter: the result is always the
        /// timeline's.
        #[test]
        fn any_delivery_order_derives_the_timeline(
            transactions in prop::collection::vec(transaction(), 1..12),
            rotation in any::<prop::sample::Index>(),
        ) {
            let transactions = unique(transactions);
            let mut shuffled = transactions.clone();
            let by = rotation.index(shuffled.len());
            shuffled.rotate_left(by);
            shuffled.reverse();

            let delivered = replay(shuffled.iter());
            prop_assert_eq!(&delivered, &replay(transactions.iter()));
            prop_assert_eq!(read(&delivered), in_timeline_order(&transactions));
        }

        /// Merging two replicas' versions equals applying everything to one.
        #[test]
        fn merge_matches_replay(
            transactions in prop::collection::vec(transaction(), 0..16),
            split in any::<prop::sample::Index>(),
        ) {
            let transactions = unique(transactions);
            let (left, right) = transactions.split_at(split.index(transactions.len() + 1));

            let mut merged = replay(left.iter());
            merged.extend(replay(right.iter()));

            prop_assert_eq!(merged, replay(transactions.iter()));
        }
    }
}
