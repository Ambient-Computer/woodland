//! Reading model data, straight from storage.
//!
//! A read doesn't take a step. Everything a step changes reaches storage
//! as one batch, so a read against a view of storage sees whole steps,
//! and never waits for one.
//!
//! # Two ways to read
//!
//! - **Optimistic** (the default): the most recent state this device knows
//!   of. It includes writes that aren't committed, so it can change in ways
//!   that look like going back: a write that was made earlier, elsewhere,
//!   can arrive later and take its place in the timeline.
//! - **Committed** ([`Query::committed`]): only what is committed. Every
//!   device that reads it sees the same history, in the same order, and it
//!   only ever moves forward. It trails the optimistic state by however
//!   long a quorum takes to decide a checkpoint. A [threshold] model
//!   is always read this way.
//!
//! # Subscribing
//!
//! The database doesn't keep queries, so it can't push results. Each
//! step says what it changed instead ([`Changed`]). Keep
//! the queries you're showing, and run again the ones a change
//! [affects]. An optimistic query feeding a signal in a UI and a
//! committed one feeding an index on disk are the same loop.
//!
//! [affects]: Query::is_affected_by
//! [threshold]: ModelId::threshold

use std::ops::{Bound, RangeBounds};

use super::table::{self, ColumnId, Value};
use super::{Kind, ModelId, kv, sequence};
use crate::db::keys::{self, Record};
use crate::db::{Changed, GroupId};
use crate::encoding;

/// A read of some of one model's entries: a single key, every key that
/// starts with a prefix, or some of a [table]'s rows.
///
/// Scan storage for each of [`Query::storage_ranges`], then decode
/// everything they return with [`Query::results`].
///
/// [table]: ModelId::table
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    group: GroupId,
    model: ModelId,
    /// The parts of the model that are wanted.
    ranges: Vec<Range>,
    /// Whether writes that aren't committed are wanted.
    optimistic: bool,
    /// A single-key query also scans keys that extend it, and drops them.
    exact: Option<Vec<u8>>,
}

/// Some of a model's keys: from the first, up to the second. `None` is
/// the end of the model.
pub(crate) type Range = (Vec<u8>, Option<Vec<u8>>);

impl Query {
    /// The value of one key.
    #[must_use]
    pub fn key(group: GroupId, model: ModelId, key: &[u8]) -> Self {
        let mut query = Self::prefix(group, model, key);
        query.exact = Some(key.to_vec());
        query
    }

    /// Every key starting with `prefix`, in key order. An empty prefix reads
    /// the whole model.
    #[must_use]
    pub fn prefix(group: GroupId, model: ModelId, prefix: &[u8]) -> Self {
        let range = (prefix.to_vec(), table::successor(prefix));

        Self::ranges(group, model, vec![range])
    }

    /// All of a [table]: decode the results with [`Table::read`].
    ///
    /// [table]: ModelId::table
    /// [`Table::read`]: crate::Table::read
    #[must_use]
    pub fn table(group: GroupId, table: ModelId) -> Self {
        Self::prefix(group, table, b"")
    }

    /// The rows of a [table] whose keys are in a range, such as
    /// `"a".."b"`, and the table's columns. Decode the results with
    /// [`Table::read`]: it's the table, with only those rows.
    ///
    /// [table]: ModelId::table
    /// [`Table::read`]: crate::Table::read
    #[must_use]
    pub fn rows<K: AsRef<[u8]>>(group: GroupId, table: ModelId, keys: impl RangeBounds<K>) -> Self {
        let bound = |bound: Bound<&K>| bound.map(|key| key.as_ref().to_vec());
        let rows = table::rows(bound(keys.start_bound()), bound(keys.end_bound()));

        Self::ranges(group, table, vec![table::schema(), rows])
    }

    /// One row of a [table], and the table's columns. Like
    /// [`Query::rows`], for a single key.
    ///
    /// [table]: ModelId::table
    #[must_use]
    pub fn row(group: GroupId, table: ModelId, key: &[u8]) -> Self {
        Self::rows(group, table, key..=key)
    }

    /// Which rows of a [table] hold a value in a range, such as
    /// `Value::from(18)..Value::from(65)`, in a column that's [indexed]
    /// or [unique]. Decode the results with [`Table::row_keys`], and
    /// read the rows themselves with [`Query::row`]. A column that's
    /// neither finds nothing.
    ///
    /// [table]: ModelId::table
    /// [indexed]: ColumnId::indexed
    /// [unique]: ColumnId::unique
    /// [`Table::row_keys`]: crate::Table::row_keys
    #[must_use]
    pub fn index(
        group: GroupId,
        table: ModelId,
        column: ColumnId,
        values: impl RangeBounds<Value>,
    ) -> Self {
        let range = table::values(column, values.start_bound(), values.end_bound());

        Self::ranges(group, table, range.into_iter().collect())
    }

    /// Which rows of a [table] hold exactly `value` in a column. Like
    /// [`Query::index`], for a single value.
    ///
    /// [table]: ModelId::table
    #[must_use]
    pub fn lookup(group: GroupId, table: ModelId, column: ColumnId, value: &Value) -> Self {
        Self::index(group, table, column, value..=value)
    }

    /// Reads only what is committed, leaving out writes that could still be
    /// reordered.
    #[must_use]
    pub fn committed(mut self) -> Self {
        self.optimistic = false;
        self
    }

    /// The ranges of storage keys to scan, each from its first key up
    /// to its second. A committed read scans one for each part of the
    /// model it wants. An optimistic one scans two, and one more.
    #[must_use]
    pub fn storage_ranges(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        let (group, model) = (self.group, self.model);
        let stored = |within: fn(GroupId, ModelId, &[u8]) -> Vec<u8>| {
            let (_, model_end) = keys::range(within(group, model, b""));
            let range = move |(start, end): &Range| {
                let end = end.as_ref().map(|end| within(group, model, end));

                (
                    within(group, model, start),
                    end.unwrap_or(model_end.clone()),
                )
            };

            self.ranges.iter().map(range)
        };

        let committed = stored(keys::committed_value);
        if !self.optimistic {
            return committed.collect();
        }

        committed.chain(stored(keys::versions)).collect()
    }

    /// Whether `change` could alter what this query returns. If so, scan
    /// and decode again.
    #[must_use]
    pub fn is_affected_by(&self, change: &Changed) -> bool {
        match change {
            Changed::Everything { group } => *group == self.group,
            Changed::Pending { .. } => false,
            Changed::Key {
                group,
                model,
                key,
                committed,
            } => {
                let ours = *group == self.group && *model == self.model;

                ours && *committed != self.optimistic && self.wants(key)
            }
        }
    }

    /// Decodes scanned storage entries into the application's keys and
    /// values, in key order. Removed keys are left out. `scanned` holds
    /// the results of every scan, in any order.
    #[must_use]
    pub fn results(&self, scanned: &[(Vec<u8>, Vec<u8>)]) -> Vec<(Vec<u8>, Vec<u8>)> {
        let ours = |group, model| group == self.group && model == self.model;
        let mut committed = Vec::new();
        let mut versions = Vec::new();
        for (stored, value) in scanned {
            match keys::parse(stored) {
                Some(Record::Committed { group, model, key })
                    if ours(group, model) && self.wants(&key) =>
                {
                    committed.push((key, value.clone()));
                }
                Some(Record::Version {
                    group,
                    model,
                    key,
                    stamp,
                }) if self.optimistic && ours(group, model) && self.wants(&key) => {
                    if let Some(value) = encoding::decode(value) {
                        versions.push((key, *stamp, value));
                    }
                }
                _ => {}
            }
        }

        if self.model.kind().is_some_and(Kind::merges) {
            return sequence::current(committed, versions).into_iter().collect();
        }

        kv::current(committed, versions).into_iter().collect()
    }

    /// Whether a key of the model is one this query asked for.
    fn wants(&self, key: &[u8]) -> bool {
        let within = |(start, end): &Range| {
            start.as_slice() <= key && end.as_ref().is_none_or(|end| key < end.as_slice())
        };

        self.exact.as_ref().is_none_or(|exact| exact == key) && self.ranges.iter().any(within)
    }

    fn ranges(group: GroupId, model: ModelId, ranges: Vec<Range>) -> Self {
        Self {
            group,
            model,
            ranges,
            optimistic: model.kind().is_some_and(Kind::shows_uncommitted),
            exact: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Stamp;
    use crate::testing::peers::{laptop, phone};

    fn group() -> GroupId {
        GroupId::from_bytes([1; 32])
    }

    fn model() -> ModelId {
        ModelId::kv(group(), [2; 32])
    }

    fn committed(key: &str, value: &str) -> (Vec<u8>, Vec<u8>) {
        let key = keys::committed_value(group(), model(), key.as_bytes());

        (key, value.into())
    }

    /// A version that isn't committed, written at `lamport`.
    fn version(lamport: u64, key: &str, value: Option<&str>) -> (Vec<u8>, Vec<u8>) {
        let author = phone();
        let stamp = Stamp { lamport, author };
        let value: Option<Vec<u8>> = value.map(Into::into);

        let key = keys::version(group(), model(), key.as_bytes(), stamp);
        (key, encoding::encode(&value))
    }

    fn result(key: &str, value: &str) -> (Vec<u8>, Vec<u8>) {
        (key.into(), value.into())
    }

    #[test]
    fn a_key_query_ignores_longer_keys() {
        let scanned = [
            committed("color", "blue"),
            committed("colors", "many"),
            version(1, "colorful", Some("very")),
        ];
        let query = Query::key(group(), model(), b"color");

        assert_eq!(query.results(&scanned), [result("color", "blue")]);
    }

    #[test]
    fn only_the_last_version_of_a_key_is_read() {
        let scanned = [
            version(1, "color", Some("red")),
            version(3, "color", Some("blue")),
            version(2, "color", None),
            version(1, "size", Some("large")),
            version(2, "size", None),
        ];
        let query = Query::prefix(group(), model(), b"");

        assert_eq!(query.results(&scanned), [result("color", "blue")]);
    }

    #[test]
    fn versions_take_the_place_of_settled_values() {
        let scanned = [
            committed("color", "red"),
            committed("size", "large"),
            committed("shape", "round"),
            version(1, "color", Some("blue")),
            version(1, "size", None),
        ];
        let query = Query::prefix(group(), model(), b"");

        assert_eq!(
            query.results(&scanned),
            [result("color", "blue"), result("shape", "round")],
        );
    }

    #[test]
    fn a_committed_query_reads_only_whats_settled() {
        let scanned = [
            committed("color", "red"),
            version(1, "color", Some("blue")),
            version(1, "size", Some("large")),
        ];
        let query = Query::prefix(group(), model(), b"").committed();

        assert_eq!(query.storage_ranges().len(), 1);
        assert_eq!(query.results(&scanned), [result("color", "red")]);
    }

    #[test]
    fn a_threshold_model_is_always_read_committed() {
        let model = ModelId::threshold(group(), [2; 32]);
        let query = Query::prefix(group(), model, b"");

        assert_eq!(query, query.clone().committed());
    }

    fn changed(key: &str, committed: bool) -> Changed {
        Changed::Key {
            group: group(),
            model: model(),
            key: key.into(),
            committed,
        }
    }

    #[test]
    fn a_query_is_affected_by_changes_to_the_keys_it_reads() {
        let one = Query::key(group(), model(), b"pet");
        let many = Query::prefix(group(), model(), b"pet");

        assert!(one.is_affected_by(&changed("pet", false)));
        assert!(!one.is_affected_by(&changed("pet/cat", false)));
        assert!(many.is_affected_by(&changed("pet/cat", false)));
        assert!(!many.is_affected_by(&changed("color", false)));

        let elsewhere = Query::key(group(), ModelId::kv(group(), [9; 32]), b"pet");
        assert!(!elsewhere.is_affected_by(&changed("pet", false)));
    }

    /// Settling a write changes committed reads. Optimistic ones showed it
    /// already.
    #[test]
    fn a_query_is_affected_by_changes_to_its_kind_of_read() {
        let optimistic = Query::key(group(), model(), b"pet");
        let committed = optimistic.clone().committed();

        assert!(optimistic.is_affected_by(&changed("pet", false)));
        assert!(!optimistic.is_affected_by(&changed("pet", true)));
        assert!(committed.is_affected_by(&changed("pet", true)));
        assert!(!committed.is_affected_by(&changed("pet", false)));
    }

    #[test]
    fn every_query_of_an_agreement_is_affected_when_everything_changed() {
        let query = Query::key(group(), model(), b"pet");
        let other = GroupId::from_bytes([2; 32]);

        assert!(query.is_affected_by(&Changed::Everything { group: group() }));
        assert!(
            query
                .committed()
                .is_affected_by(&Changed::Everything { group: group() })
        );
        assert!(
            !Query::key(other, model(), b"pet")
                .is_affected_by(&Changed::Everything { group: group() })
        );
    }

    /// A stamp ends in its author's key, which can hold any bytes. A key
    /// must never be mistaken for a shorter key followed by a stamp.
    #[test]
    fn stamps_arent_mistaken_for_keys() {
        let stamp = Stamp {
            lamport: 1,
            author: laptop(),
        };
        let long = keys::version(group(), model(), b"color", stamp);
        let long = &long[keys::versions(group(), model(), b"").len()..];

        let scanned = [version(1, "color", Some("blue"))];
        let query = Query::key(group(), model(), long);

        assert_eq!(query.results(&scanned), []);
    }

    #[test]
    fn a_prefix_query_leaves_out_removed_keys() {
        let scanned = [
            committed("pet/cat", "tabby"),
            version(1, "pet/dog", None),
            version(1, "pet/fish", Some("goldfish")),
        ];
        let query = Query::prefix(group(), model(), b"pet/");

        assert_eq!(
            query.results(&scanned),
            [result("pet/cat", "tabby"), result("pet/fish", "goldfish")],
        );
    }
}
