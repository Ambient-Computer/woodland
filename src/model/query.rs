//! Reading model data, straight from storage.
//!
//! Reads don't go through the [`Node`]. Every change the node integrates is
//! written to storage in the same batch, and writes come before any other
//! action, so once the application has performed a batch's actions,
//! storage holds the latest state. Reading it directly means a read never
//! waits on the state machine, and the application can use whatever its
//! storage offers: its own thread, snapshots, cursors.
//!
//! What it gives up: the node can't say what changed, so there's nothing
//! here to subscribe to. That would be a change feed, as a separate kind of
//! action, rather than a different read path.
//!
//! [`Node`]: crate::Node

use super::ModelId;
use super::kv::{self, Version};
use crate::agreement::AgreementId;
use crate::encoding;
use crate::storage::keys::{self, Record};

/// A read of one model's entries: a single key, or every key that starts
/// with a prefix.
///
/// Scan storage for [`Query::storage_prefix`], then decode what it returns
/// with [`Query::results`]. Storage holds every version of a key that
/// hasn't been compacted, and the results hold only the current one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    storage_prefix: Vec<u8>,
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

    #[must_use]
    pub fn storage_prefix(&self) -> &[u8] {
        &self.storage_prefix
    }

    /// Decodes scanned storage entries into the application's keys and
    /// values, in key order. Removed keys are left out.
    #[must_use]
    pub fn results(&self, scanned: &[(Vec<u8>, Vec<u8>)]) -> Vec<(Vec<u8>, Vec<u8>)> {
        let versions = scanned
            .iter()
            .filter(|(key, _)| key.starts_with(&self.storage_prefix))
            .filter_map(|(key, value)| {
                let Record::Version { key, stamp, .. } = keys::parse(key)? else {
                    return None;
                };

                Some((key, *stamp, encoding::decode::<Version>(value)?))
            })
            .filter(|(key, ..)| self.exact.as_ref().is_none_or(|exact| exact == key));

        kv::current(versions).into_iter().collect()
    }

    fn new(agreement: AgreementId, model: ModelId, key: &[u8], exact: bool) -> Self {
        Self {
            storage_prefix: keys::versions(agreement, model, key),
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
        ModelId::new(agreement(), [2; 32])
    }

    fn stored(key: &str, value: Option<&str>) -> (Vec<u8>, Vec<u8>) {
        stored_at(1, key, value)
    }

    fn stored_at(lamport: u64, key: &str, value: Option<&str>) -> (Vec<u8>, Vec<u8>) {
        let author = phone();
        let stamp = Stamp { lamport, author };
        let version = Version(value.map(Into::into));

        let key = keys::version(agreement(), model(), key.as_bytes(), stamp);
        (key, encoding::encode(&version))
    }

    fn result(key: &str, value: &str) -> (Vec<u8>, Vec<u8>) {
        (key.into(), value.into())
    }

    #[test]
    fn a_key_query_ignores_longer_keys() {
        let scanned = [
            stored("color", Some("blue")),
            stored("colors", Some("many")),
        ];
        let query = Query::key(agreement(), model(), b"color");

        assert_eq!(query.results(&scanned), [result("color", "blue")]);
    }

    #[test]
    fn only_the_last_version_of_a_key_is_read() {
        let mut scanned = vec![
            stored_at(1, "color", Some("red")),
            stored_at(3, "color", Some("blue")),
            stored_at(2, "color", None),
            stored_at(1, "size", Some("large")),
            stored_at(2, "size", None),
        ];
        scanned.sort();
        let query = Query::prefix(agreement(), model(), b"");

        assert_eq!(query.results(&scanned), [result("color", "blue")]);
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
        let long = &long[keys::model(agreement(), model()).len()..];

        let scanned = [stored("color", Some("blue"))];
        let query = Query::key(agreement(), model(), long);

        assert_eq!(query.results(&scanned), []);
    }

    #[test]
    fn a_prefix_query_leaves_out_removed_keys() {
        let scanned = [
            stored("pet/cat", Some("tabby")),
            stored("pet/dog", None),
            stored("pet/fish", Some("goldfish")),
        ];
        let query = Query::prefix(agreement(), model(), b"pet/");

        assert_eq!(
            query.results(&scanned),
            [result("pet/cat", "tabby"), result("pet/fish", "goldfish")],
        );
    }
}
