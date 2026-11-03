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

use super::{Entry, ModelId};
use crate::agreement::AgreementId;
use crate::encoding;
use crate::storage::keys;

/// A read of one model's entries: a single key, or every key that starts
/// with a prefix.
///
/// Scan storage for [`Query::storage_prefix`], then decode what it returns
/// with [`Query::results`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    storage_prefix: Vec<u8>,
    /// Where the application's key starts within a storage key.
    key_offset: usize,
    /// A single-key query also scans keys that extend it, and drops them.
    exact: bool,
}

impl Query {
    /// The value of one key. Its storage prefix is the exact storage key,
    /// so a point read works as well as a scan.
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
        scanned
            .iter()
            .filter(|(key, _)| key.starts_with(&self.storage_prefix))
            .filter(|(key, _)| !self.exact || key.len() == self.storage_prefix.len())
            .filter_map(|(key, value)| {
                let value = encoding::decode::<Entry>(value)?.value?;
                Some((key[self.key_offset..].to_vec(), value))
            })
            .collect()
    }

    fn new(agreement: AgreementId, model: ModelId, key: &[u8], exact: bool) -> Self {
        Self {
            storage_prefix: keys::entry(agreement, model, key),
            key_offset: keys::model(agreement, model).len(),
            exact,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agreement::Genesis;
    use crate::clock::Stamp;
    use crate::testing::peers::phone;

    fn agreement() -> AgreementId {
        Genesis::new([1; 32], [phone().id()]).id()
    }

    fn model() -> ModelId {
        ModelId::new(agreement(), [2; 32])
    }

    fn stored(key: &str, value: Option<&str>) -> (Vec<u8>, Vec<u8>) {
        let entry = Entry {
            value: value.map(Into::into),
            stamp: Stamp {
                lamport: 1,
                author: phone(),
            },
        };

        let key = keys::entry(agreement(), model(), key.as_bytes());
        (key, encoding::encode(&entry))
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
