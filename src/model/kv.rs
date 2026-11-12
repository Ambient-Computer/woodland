//! A key-value map, derived from the timeline: each key holds whatever the
//! last transaction to write it left there.
//!
//! # Versions
//!
//! Every write is first kept as a version of its key, under its
//! transaction's [`Stamp`]. Stamps are the timeline's order, so a key's
//! versions, sorted by stamp, are that key's recent history, and the last
//! one is its current value.
//!
//! That makes writing blind. A transaction's versions go straight to
//! storage, wherever they land in the timeline, without reading what's
//! already there. A write that arrives late, with an older stamp, slots in
//! behind the newer ones and changes nothing a reader sees.
//!
//! Removing a key writes a version with no value. It's what makes a late,
//! older `Set` lose to a newer `Remove` it never saw.
//!
//! # Settling
//!
//! Once a transaction settles, nothing can land before it in the timeline
//! anymore (see `group::settling`). Its versions are folded, in timeline
//! order, into the one settled value each key has, and a settled removal
//! simply leaves no value. So what's kept per key is one settled value,
//! plus a version for each write that hasn't settled yet.
//!
//! # Reading
//!
//! A key's current value is its last version, or its settled value if it
//! has no versions. Reading only settled values gives the state every
//! device agrees on, and will never disagree on.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::Write;
use crate::clock::Stamp;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Operation {
    Set { key: Vec<u8>, value: Vec<u8> },
    Remove { key: Vec<u8> },
}

impl Operation {
    /// What the operation's key holds once it's applied.
    pub(crate) fn write(&self) -> Write {
        match self {
            Self::Set { key, value } => Write {
                key: key.clone(),
                value: Some(value.clone()),
            },
            Self::Remove { key } => Write {
                key: key.clone(),
                value: None,
            },
        }
    }
}

/// The current value of every key that has one: its last version in the
/// timeline, or its settled value if nothing has written it since.
pub(crate) fn current(
    settled: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
    versions: impl IntoIterator<Item = (Vec<u8>, Stamp, Option<Vec<u8>>)>,
) -> BTreeMap<Vec<u8>, Vec<u8>> {
    let mut latest: BTreeMap<Vec<u8>, (Stamp, Option<Vec<u8>>)> = BTreeMap::new();
    for (key, stamp, value) in versions {
        let newer = latest.get(&key).is_none_or(|(held, _)| *held <= stamp);
        if newer {
            latest.insert(key, (stamp, value));
        }
    }

    let mut current: BTreeMap<Vec<u8>, Vec<u8>> = settled.into_iter().collect();
    for (key, (_, value)) in latest {
        match value {
            Some(value) => current.insert(key, value),
            None => current.remove(&key),
        };
    }

    current
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::actor::Peer;
    use crate::testing::peers::{laptop, nas, phone};

    /// Plays storage: every settled value, and every version by key and
    /// stamp. Like storage, a later write to the same place replaces the
    /// earlier one.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct Store {
        settled: BTreeMap<Vec<u8>, Vec<u8>>,
        versions: BTreeMap<(Vec<u8>, Stamp), Option<Vec<u8>>>,
    }

    impl Store {
        /// Integrates one transaction's operations, all under its stamp.
        fn apply(&mut self, stamp: Stamp, operations: &[Operation]) {
            for Write { key, value } in operations.iter().map(Operation::write) {
                self.versions.insert((key, stamp), value);
            }
        }

        /// Settles one transaction, the way the group does: its writes are
        /// folded into the settled values, and its versions dropped.
        fn settle(&mut self, stamp: Stamp, operations: &[Operation]) {
            for Write { key, value } in operations.iter().map(Operation::write) {
                self.versions.remove(&(key.clone(), stamp));
                match value {
                    Some(value) => self.settled.insert(key, value),
                    None => self.settled.remove(&key),
                };
            }
        }

        fn read(&self) -> BTreeMap<Vec<u8>, Vec<u8>> {
            let versions = self.versions.clone().into_iter();

            current(
                self.settled.clone(),
                versions.map(|((key, stamp), value)| (key, stamp, value)),
            )
        }

        fn get(&self, key: &str) -> Option<String> {
            let value = self.read().remove(key.as_bytes())?;

            Some(String::from_utf8_lossy(&value).into_owned())
        }
    }

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
        let mut store = Store::default();
        store.apply(stamp(2, phone()), &[set("color", "blue")]);
        store.apply(stamp(1, laptop()), &[set("color", "red")]);

        assert_eq!(store.get("color").as_deref(), Some("blue"));
    }

    #[test]
    fn concurrent_writes_break_ties_by_author() {
        let (low, high) = if phone() < laptop() {
            (phone(), laptop())
        } else {
            (laptop(), phone())
        };

        let mut store = Store::default();
        store.apply(stamp(1, high), &[set("color", "high")]);
        store.apply(stamp(1, low), &[set("color", "low")]);

        assert_eq!(store.get("color").as_deref(), Some("high"));
    }

    #[test]
    fn removals_beat_older_writes() {
        let mut store = Store::default();
        store.apply(stamp(2, phone()), &[remove("color")]);
        store.apply(stamp(1, laptop()), &[set("color", "red")]);

        assert_eq!(store.get("color"), None);
    }

    #[test]
    fn later_operations_in_a_transaction_win() {
        let mut store = Store::default();
        let operations = [set("color", "red"), remove("color")];
        store.apply(stamp(1, phone()), &operations);

        assert_eq!(store.get("color"), None);
    }

    #[test]
    fn an_unsettled_removal_hides_the_settled_value() {
        let mut store = Store::default();
        store.apply(stamp(1, phone()), &[set("color", "red")]);
        store.settle(stamp(1, phone()), &[set("color", "red")]);
        store.apply(stamp(2, laptop()), &[remove("color")]);

        assert_eq!(store.get("color"), None);
        assert_eq!(store.settled.get(&b"color"[..]), Some(&b"red".to_vec()));
    }

    #[test]
    fn a_settled_removal_leaves_nothing_behind() {
        let mut store = Store::default();
        let history = [
            (stamp(1, phone()), [set("color", "red")]),
            (stamp(2, laptop()), [remove("color")]),
        ];
        for (stamp, operations) in &history {
            store.apply(*stamp, operations);
        }
        for (stamp, operations) in &history {
            store.settle(*stamp, operations);
        }

        assert_eq!(store, Store::default());
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
    /// so keep the first of each. The result is in timeline order.
    fn timeline(transactions: Vec<(Stamp, Vec<Operation>)>) -> Vec<(Stamp, Vec<Operation>)> {
        let mut unique = BTreeMap::new();
        for (stamp, operations) in transactions {
            unique.entry(stamp).or_insert(operations);
        }

        unique.into_iter().collect()
    }

    fn replay<'a>(transactions: impl Iterator<Item = &'a (Stamp, Vec<Operation>)>) -> Store {
        let mut store = Store::default();
        for (stamp, operations) in transactions {
            store.apply(*stamp, operations);
        }

        store
    }

    /// What applying the transactions one at a time, in order, leaves
    /// behind: the plainest reading of "derived from the timeline".
    fn one_at_a_time(transactions: &[(Stamp, Vec<Operation>)]) -> BTreeMap<Vec<u8>, Vec<u8>> {
        let mut state = BTreeMap::new();
        for Write { key, value } in transactions
            .iter()
            .flat_map(|(_, operations)| operations.iter().map(Operation::write))
        {
            match value {
                Some(value) => state.insert(key, value),
                None => state.remove(&key),
            };
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
            let transactions = timeline(transactions);
            let mut shuffled = transactions.clone();
            let by = rotation.index(shuffled.len());
            shuffled.rotate_left(by);
            shuffled.reverse();

            let delivered = replay(shuffled.iter());
            prop_assert_eq!(&delivered, &replay(transactions.iter()));
            prop_assert_eq!(delivered.read(), one_at_a_time(&transactions));
        }

        /// Settling a prefix of the timeline changes what's stored, never
        /// what's read. And the settled values are what the prefix alone
        /// derives.
        #[test]
        fn settling_a_prefix_changes_nothing_a_reader_sees(
            transactions in prop::collection::vec(transaction(), 1..12),
            prefix in any::<prop::sample::Index>(),
        ) {
            let transactions = timeline(transactions);
            let (settled, _) = transactions.split_at(prefix.index(transactions.len() + 1));

            let mut store = replay(transactions.iter());
            for (stamp, operations) in settled {
                store.settle(*stamp, operations);
            }

            prop_assert_eq!(store.read(), one_at_a_time(&transactions));
            prop_assert_eq!(store.settled, one_at_a_time(settled));
        }
    }
}
