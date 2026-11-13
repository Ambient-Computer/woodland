//! Reading model data, straight from storage.
//!
//! Reads don't go through the [`Node`]. Every change the node integrates is
//! written to storage in the same batch, and writes come before any other
//! action, so once the application has performed a batch's actions,
//! storage holds the latest state. Reading it directly means a read never
//! waits on the state machine, and the application can use whatever its
//! storage offers: its own thread, snapshots, cursors.
//!
//! # Two ways to read
//!
//! - **Optimistic** (the default): the most recent state this device knows
//!   of. It includes writes that haven't settled, so it can change in ways
//!   that look like going back: a write that was made earlier, elsewhere,
//!   can arrive later and take its place in the timeline.
//! - **Committed** ([`Query::committed`]): only what has settled. Every
//!   device that reads it sees the same history, in the same order, and it
//!   only ever moves forward. It trails the optimistic state by however
//!   long the slowest contributor takes to catch up. A [threshold] model
//!   is always read this way.
//!
//! # Subscribing
//!
//! The node doesn't run queries, so it can't push results. It says what
//! changed instead ([`Changed`]), after the write that changed it. Keep
//! the queries you're showing, and run again the ones a change
//! [affects]. An optimistic query feeding a signal in a UI and a
//! committed one feeding an index on disk are the same loop.
//!
//! [`Node`]: crate::Node
//! [affects]: Query::is_affected_by
//! [threshold]: ModelId::threshold

use std::ops::{Bound, RangeBounds};

use super::table::{self, ColumnId, Value};
use super::{Kind, ModelId, kv};
use crate::agreement::AgreementId;
use crate::encoding;
use crate::io::Changed;
use crate::storage::keys::{self, Record};

/// A read of some of one model's entries: a single key, every key that
/// starts with a prefix, or some of a [table]'s rows.
///
/// Scan storage for each of [`Query::storage_ranges`], then decode
/// everything they return with [`Query::results`].
///
/// [table]: ModelId::table
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    agreement: AgreementId,
    model: ModelId,
    /// The parts of the model that are wanted.
    ranges: Vec<Range>,
    /// Whether writes that haven't settled are wanted.
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
    pub fn key(agreement: AgreementId, model: ModelId, key: &[u8]) -> Self {
        let mut query = Self::prefix(agreement, model, key);
        query.exact = Some(key.to_vec());
        query
    }

    /// Every key starting with `prefix`, in key order. An empty prefix reads
    /// the whole model.
    #[must_use]
    pub fn prefix(agreement: AgreementId, model: ModelId, prefix: &[u8]) -> Self {
        let range = (prefix.to_vec(), table::successor(prefix));

        Self::ranges(agreement, model, vec![range])
    }

    /// All of a [table]: decode the results with [`Table::read`].
    ///
    /// [table]: ModelId::table
    /// [`Table::read`]: crate::Table::read
    #[must_use]
    pub fn table(agreement: AgreementId, table: ModelId) -> Self {
        Self::prefix(agreement, table, b"")
    }

    /// The rows of a [table] whose keys are in a range, such as
    /// `"a".."b"`, and the table's columns. Decode the results with
    /// [`Table::read`]: it's the table, with only those rows.
    ///
    /// [table]: ModelId::table
    /// [`Table::read`]: crate::Table::read
    #[must_use]
    pub fn rows<K: AsRef<[u8]>>(
        agreement: AgreementId,
        table: ModelId,
        keys: impl RangeBounds<K>,
    ) -> Self {
        let bound = |bound: Bound<&K>| bound.map(|key| key.as_ref().to_vec());
        let rows = table::rows(bound(keys.start_bound()), bound(keys.end_bound()));

        Self::ranges(agreement, table, vec![table::schema(), rows])
    }

    /// One row of a [table], and the table's columns. Like
    /// [`Query::rows`], for a single key.
    ///
    /// [table]: ModelId::table
    #[must_use]
    pub fn row(agreement: AgreementId, table: ModelId, key: &[u8]) -> Self {
        Self::rows(agreement, table, key..=key)
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
        agreement: AgreementId,
        table: ModelId,
        column: ColumnId,
        values: impl RangeBounds<Value>,
    ) -> Self {
        let range = table::values(column, values.start_bound(), values.end_bound());

        Self::ranges(agreement, table, range.into_iter().collect())
    }

    /// Which rows of a [table] hold exactly `value` in a column. Like
    /// [`Query::index`], for a single value.
    ///
    /// [table]: ModelId::table
    #[must_use]
    pub fn lookup(agreement: AgreementId, table: ModelId, column: ColumnId, value: &Value) -> Self {
        Self::index(agreement, table, column, value..=value)
    }

    /// Reads only what has settled, leaving out writes that could still be
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
        let (agreement, model) = (self.agreement, self.model);
        let stored = |within: fn(AgreementId, ModelId, &[u8]) -> Vec<u8>| {
            let (_, model_end) = keys::range(within(agreement, model, b""));
            let range = move |(start, end): &Range| {
                let end = end.as_ref().map(|end| within(agreement, model, end));

                (
                    within(agreement, model, start),
                    end.unwrap_or(model_end.clone()),
                )
            };

            self.ranges.iter().map(range)
        };

        let settled = stored(keys::settled_value);
        if !self.optimistic {
            return settled.collect();
        }

        let drops = keys::range(keys::drops(agreement, model));

        settled
            .chain(stored(keys::versions))
            .chain([drops])
            .collect()
    }

    /// Whether `change` could alter what this query returns. If so, scan
    /// and decode again.
    #[must_use]
    pub fn is_affected_by(&self, change: &Changed) -> bool {
        match change {
            Changed::Everything { agreement } => *agreement == self.agreement,
            Changed::Key {
                agreement,
                model,
                key,
                committed,
            } => {
                let ours = *agreement == self.agreement && *model == self.model;

                ours && *committed != self.optimistic && self.wants(key)
            }
        }
    }

    /// Decodes scanned storage entries into the application's keys and
    /// values, in key order. Removed keys are left out. `scanned` holds
    /// the results of every scan, in any order.
    #[must_use]
    pub fn results(&self, scanned: &[(Vec<u8>, Vec<u8>)]) -> Vec<(Vec<u8>, Vec<u8>)> {
        // A dropped model is empty, whatever it still holds in storage.
        let drops = keys::drops(self.agreement, self.model);
        let dropped = |(stored, _): &(Vec<u8>, Vec<u8>)| stored.starts_with(&drops);
        if self.optimistic && scanned.iter().any(dropped) {
            return Vec::new();
        }

        let ours = |agreement, model| agreement == self.agreement && model == self.model;
        let mut settled = Vec::new();
        let mut versions = Vec::new();
        for (stored, value) in scanned {
            match keys::parse(stored) {
                Some(Record::Settled {
                    agreement,
                    model,
                    key,
                }) if ours(agreement, model) && self.wants(&key) => {
                    settled.push((key, value.clone()));
                }
                Some(Record::Version {
                    agreement,
                    model,
                    key,
                    stamp,
                }) if self.optimistic && ours(agreement, model) && self.wants(&key) => {
                    if let Some(value) = encoding::decode(value) {
                        versions.push((key, *stamp, value));
                    }
                }
                _ => {}
            }
        }

        kv::current(settled, versions).into_iter().collect()
    }

    /// Whether a key of the model is one this query asked for.
    fn wants(&self, key: &[u8]) -> bool {
        let within = |(start, end): &Range| {
            start.as_slice() <= key && end.as_ref().is_none_or(|end| key < end.as_slice())
        };

        self.exact.as_ref().is_none_or(|exact| exact == key) && self.ranges.iter().any(within)
    }

    fn ranges(agreement: AgreementId, model: ModelId, ranges: Vec<Range>) -> Self {
        Self {
            agreement,
            model,
            ranges,
            optimistic: model.kind().is_some_and(Kind::shows_unsettled),
            exact: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agreement::Genesis;
    use crate::clock::Stamp;
    use crate::testing::peers::{laptop, phone};

    fn agreement() -> AgreementId {
        Genesis::new([1; 32], [phone().id()]).id()
    }

    fn model() -> ModelId {
        ModelId::kv(agreement(), [2; 32])
    }

    fn settled(key: &str, value: &str) -> (Vec<u8>, Vec<u8>) {
        let key = keys::settled_value(agreement(), model(), key.as_bytes());

        (key, value.into())
    }

    /// A version that hasn't settled, written at `lamport`.
    fn version(lamport: u64, key: &str, value: Option<&str>) -> (Vec<u8>, Vec<u8>) {
        let author = phone();
        let stamp = Stamp { lamport, author };
        let value: Option<Vec<u8>> = value.map(Into::into);

        let key = keys::version(agreement(), model(), key.as_bytes(), stamp);
        (key, encoding::encode(&value))
    }

    fn result(key: &str, value: &str) -> (Vec<u8>, Vec<u8>) {
        (key.into(), value.into())
    }

    #[test]
    fn a_key_query_ignores_longer_keys() {
        let scanned = [
            settled("color", "blue"),
            settled("colors", "many"),
            version(1, "colorful", Some("very")),
        ];
        let query = Query::key(agreement(), model(), b"color");

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
        let query = Query::prefix(agreement(), model(), b"");

        assert_eq!(query.results(&scanned), [result("color", "blue")]);
    }

    #[test]
    fn versions_take_the_place_of_settled_values() {
        let scanned = [
            settled("color", "red"),
            settled("size", "large"),
            settled("shape", "round"),
            version(1, "color", Some("blue")),
            version(1, "size", None),
        ];
        let query = Query::prefix(agreement(), model(), b"");

        assert_eq!(
            query.results(&scanned),
            [result("color", "blue"), result("shape", "round")],
        );
    }

    #[test]
    fn a_committed_query_reads_only_whats_settled() {
        let scanned = [
            settled("color", "red"),
            version(1, "color", Some("blue")),
            version(1, "size", Some("large")),
        ];
        let query = Query::prefix(agreement(), model(), b"").committed();

        assert_eq!(query.storage_ranges().len(), 1);
        assert_eq!(query.results(&scanned), [result("color", "red")]);
    }

    #[test]
    fn a_dropped_model_reads_as_empty_before_the_drop_settles() {
        let stamp = Stamp {
            lamport: 2,
            author: phone(),
        };
        let dropped = (keys::drop(agreement(), model(), stamp), Vec::new());
        let scanned = [
            settled("color", "red"),
            version(1, "size", Some("large")),
            dropped,
        ];
        let query = Query::prefix(agreement(), model(), b"");

        assert_eq!(query.results(&scanned), []);
        assert_eq!(
            query.committed().results(&scanned),
            [result("color", "red")]
        );
    }

    #[test]
    fn a_threshold_model_is_always_read_committed() {
        let model = ModelId::threshold(agreement(), [2; 32]);
        let query = Query::prefix(agreement(), model, b"");

        assert_eq!(query, query.clone().committed());
    }

    fn changed(key: &str, committed: bool) -> Changed {
        Changed::Key {
            agreement: agreement(),
            model: model(),
            key: key.into(),
            committed,
        }
    }

    #[test]
    fn a_query_is_affected_by_changes_to_the_keys_it_reads() {
        let one = Query::key(agreement(), model(), b"pet");
        let many = Query::prefix(agreement(), model(), b"pet");

        assert!(one.is_affected_by(&changed("pet", false)));
        assert!(!one.is_affected_by(&changed("pet/cat", false)));
        assert!(many.is_affected_by(&changed("pet/cat", false)));
        assert!(!many.is_affected_by(&changed("color", false)));

        let elsewhere = Query::key(agreement(), ModelId::kv(agreement(), [9; 32]), b"pet");
        assert!(!elsewhere.is_affected_by(&changed("pet", false)));
    }

    /// Settling a write changes committed reads. Optimistic ones showed it
    /// already.
    #[test]
    fn a_query_is_affected_by_changes_to_its_kind_of_read() {
        let optimistic = Query::key(agreement(), model(), b"pet");
        let committed = optimistic.clone().committed();

        assert!(optimistic.is_affected_by(&changed("pet", false)));
        assert!(!optimistic.is_affected_by(&changed("pet", true)));
        assert!(committed.is_affected_by(&changed("pet", true)));
        assert!(!committed.is_affected_by(&changed("pet", false)));
    }

    #[test]
    fn every_query_of_an_agreement_is_affected_when_everything_changed() {
        let query = Query::key(agreement(), model(), b"pet");
        let other = Genesis::new([2; 32], [phone().id()]).id();

        assert!(query.is_affected_by(&Changed::Everything {
            agreement: agreement()
        }));
        assert!(query.committed().is_affected_by(&Changed::Everything {
            agreement: agreement()
        }));
        assert!(
            !Query::key(other, model(), b"pet").is_affected_by(&Changed::Everything {
                agreement: agreement()
            })
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
        let long = keys::version(agreement(), model(), b"color", stamp);
        let long = &long[keys::versions(agreement(), model(), b"").len()..];

        let scanned = [version(1, "color", Some("blue"))];
        let query = Query::key(agreement(), model(), long);

        assert_eq!(query.results(&scanned), []);
    }

    #[test]
    fn a_prefix_query_leaves_out_removed_keys() {
        let scanned = [
            settled("pet/cat", "tabby"),
            version(1, "pet/dog", None),
            version(1, "pet/fish", Some("goldfish")),
        ];
        let query = Query::prefix(agreement(), model(), b"pet/");

        assert_eq!(
            query.results(&scanned),
            [result("pet/cat", "tabby"), result("pet/fish", "goldfish")],
        );
    }
}
