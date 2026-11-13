//! An in-memory storage backend: the whole contract in a `BTreeMap`.

use std::collections::BTreeMap;

use crate::storage::StorageWrite;

#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryStore {
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl MemoryStore {
    /// Applies a write. A map in memory is trivially atomic and durable.
    pub(crate) fn write(&mut self, writes: &[StorageWrite]) {
        for write in writes {
            match write {
                StorageWrite::Put { key, value } => {
                    self.entries.insert(key.clone(), value.clone());
                }

                StorageWrite::Delete { key } => {
                    self.entries.remove(key);
                }

                StorageWrite::DeleteRange { start, end } => {
                    let doomed: Vec<_> = self
                        .entries
                        .range(start.clone()..end.clone())
                        .map(|(key, _)| key.clone())
                        .collect();

                    for key in doomed {
                        self.entries.remove(&key);
                    }
                }
            }
        }
    }

    pub(crate) fn scan(&self, prefix: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.entries
            .range(prefix.to_vec()..)
            .take_while(|(key, _)| key.starts_with(prefix))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// The first `limit` entries in `start..end`, or all of them.
    pub(crate) fn scan_range(
        &self,
        start: &[u8],
        end: &[u8],
        limit: Option<usize>,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.entries
            .range(start.to_vec()..end.to_vec())
            .take(limit.unwrap_or(usize::MAX))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    pub(crate) fn get(&self, keys: Vec<Vec<u8>>) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
        let read = |key: Vec<u8>| {
            let value = self.entries.get(&key).cloned();

            (key, value)
        };

        keys.into_iter().map(read).collect()
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &Vec<u8>> {
        self.entries.keys()
    }
}
