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
//! What it gives up: the node can't say what changed, so there's nothing
//! here to subscribe to. That would be a change feed, as a separate kind of
//! action, rather than a different read path.
//!
//! [`Node`]: crate::Node
//! [threshold]: ModelId::threshold

use super::kv;
use super::{Kind, ModelId};
use crate::agreement::AgreementId;
use crate::encoding;
use crate::storage::keys::{self, Record};

/// A read of one model's entries: a single key, or every key that starts
/// with a prefix.
///
/// Scan storage for each of [`Query::storage_prefixes`], then decode
/// everything they return with [`Query::results`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// Where the settled values are.
    settled: Vec<u8>,
    /// Where the versions that haven't settled are, if they're wanted.
    frontier: Option<Vec<u8>>,
    /// A single-key query also scans keys that extend it, and drops them.
    exact: Option<Vec<u8>>,
}

impl Query {
    /// The value of one key.
    #[must_use]
    pub fn key(agreement: AgreementId, model: ModelId, key: &[u8]) -> Self {
        Self::new(agreement, model, key, true)
    }

    /// Every key starting with `prefix`, in key order. An empty prefix reads
    /// the whole model.
    #[must_use]
    pub fn prefix(agreement: AgreementId, model: ModelId, prefix: &[u8]) -> Self {
        Self::new(agreement, model, prefix, false)
    }

    /// Reads only what has settled, leaving out writes that could still be
    /// reordered.
    #[must_use]
    pub fn committed(mut self) -> Self {
        self.frontier = None;
        self
    }

    /// The storage prefixes to scan: one for a committed read, two for an
    /// optimistic one.
    pub fn storage_prefixes(&self) -> impl Iterator<Item = &[u8]> {
        let frontier = self.frontier.as_deref();

        std::iter::once(self.settled.as_slice()).chain(frontier)
    }

    /// Decodes scanned storage entries into the application's keys and
    /// values, in key order. Removed keys are left out. `scanned` holds
    /// the results of every scan, in any order.
    #[must_use]
    pub fn results(&self, scanned: &[(Vec<u8>, Vec<u8>)]) -> Vec<(Vec<u8>, Vec<u8>)> {
        let wanted = |key: &Vec<u8>| self.exact.as_ref().is_none_or(|exact| exact == key);
        let mut settled = Vec::new();
        let mut versions = Vec::new();
        for (stored, value) in scanned.iter().filter(|(key, _)| self.scans(key)) {
            match keys::parse(stored) {
                Some(Record::Settled { key, .. }) if wanted(&key) => {
                    settled.push((key, value.clone()));
                }
                Some(Record::Version { key, stamp, .. }) if wanted(&key) => {
                    if let Some(value) = encoding::decode(value) {
                        versions.push((key, *stamp, value));
                    }
                }
                _ => {}
            }
        }

        kv::current(settled, versions).into_iter().collect()
    }

    /// Whether a stored key is one this query asked for.
    fn scans(&self, stored: &[u8]) -> bool {
        self.storage_prefixes()
            .any(|prefix| stored.starts_with(prefix))
    }

    fn new(agreement: AgreementId, model: ModelId, key: &[u8], exact: bool) -> Self {
        let shows_unsettled = model.kind().is_some_and(Kind::shows_unsettled);
        let frontier = shows_unsettled.then(|| keys::versions(agreement, model, key));

        Self {
            settled: keys::settled_value(agreement, model, key),
            frontier,
            exact: exact.then(|| key.to_vec()),
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

        assert_eq!(query.storage_prefixes().count(), 1);
        assert_eq!(query.results(&scanned), [result("color", "red")]);
    }

    #[test]
    fn a_threshold_model_is_always_read_committed() {
        let model = ModelId::threshold(agreement(), [2; 32]);
        let query = Query::prefix(agreement(), model, b"");

        assert_eq!(query, query.clone().committed());
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
