use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;

use super::{Entry, Order, Store, StoreError, View, Write};

type Entries = BTreeMap<Vec<u8>, Vec<u8>>;

/// The whole contract in a map in memory, for tests and simulations.
///
/// It keeps nothing when the process ends, so a device that uses it for
/// real is a new device every time it starts.
#[derive(Clone, Debug, Default)]
pub struct MemoryStore {
    /// Shared with every view taken since the last batch. A batch copies
    /// it only if a view is still holding the old one.
    entries: Arc<Entries>,
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Store for MemoryStore {
    fn apply(&mut self, batch: Vec<Write>) -> Result<(), StoreError> {
        let entries = Arc::make_mut(&mut self.entries);
        for write in batch {
            match write {
                Write::Put { key, value } => entries.insert(key, value),
                Write::Delete { key } => entries.remove(&key),
            };
        }

        Ok(())
    }

    fn view(&self) -> Result<Box<dyn View>, StoreError> {
        Ok(Box::new(MemoryView(Arc::clone(&self.entries))))
    }
}

#[derive(Debug)]
struct MemoryView(Arc<Entries>);

impl View for MemoryView {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.0.get(key).cloned())
    }

    fn scan(
        &self,
        start: &[u8],
        end: Option<&[u8]>,
        order: Order,
        limit: usize,
    ) -> Result<Vec<Entry>, StoreError> {
        if end.is_some_and(|end| end <= start) {
            return Ok(Vec::new());
        }

        let end = end.map_or(Bound::Unbounded, Bound::Excluded);
        let range = self.0.range::<[u8], _>((Bound::Included(start), end));
        let owned = |(key, value): (&Vec<u8>, &Vec<u8>)| (key.clone(), value.clone());

        Ok(match order {
            Order::Ascending => range.take(limit).map(owned).collect(),
            Order::Descending => range.rev().take(limit).map(owned).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTHING: [Vec<u8>; 0] = [];

    fn put(key: &[u8], value: &[u8]) -> Write {
        Write::Put {
            key: key.to_vec(),
            value: value.to_vec(),
        }
    }

    fn delete(key: &[u8]) -> Write {
        Write::Delete { key: key.to_vec() }
    }

    fn store(keys: &[&[u8]]) -> MemoryStore {
        let mut store = MemoryStore::new();
        let batch = keys.iter().map(|key| put(key, key)).collect();
        store.apply(batch).expect("memory doesn't fail");

        store
    }

    fn keys(store: &MemoryStore, start: &[u8], end: Option<&[u8]>, order: Order) -> Vec<Vec<u8>> {
        let view = store.view().expect("memory doesn't fail");
        let entries = view.scan(start, end, order, 2);

        entries
            .expect("memory doesn't fail")
            .into_iter()
            .map(|(key, _)| key)
            .collect()
    }

    #[test]
    fn a_batch_applies_in_order() {
        let mut store = MemoryStore::new();
        let batch = vec![
            put(b"a", b"1"),
            put(b"a", b"2"),
            put(b"b", b"1"),
            delete(b"b"),
        ];

        store.apply(batch).expect("memory doesn't fail");

        let view = store.view().expect("memory doesn't fail");
        assert_eq!(view.get(b"a"), Ok(Some(b"2".to_vec())));
        assert_eq!(view.get(b"b"), Ok(None));
    }

    #[test]
    fn a_view_doesnt_see_what_came_after_it() {
        let mut store = store(&[b"a"]);
        let before = store.view().expect("memory doesn't fail");

        store
            .apply(vec![delete(b"a"), put(b"b", b"b")])
            .expect("memory doesn't fail");

        assert_eq!(before.get(b"a"), Ok(Some(b"a".to_vec())));
        assert_eq!(before.get(b"b"), Ok(None));
        let after = store.view().expect("memory doesn't fail");
        assert_eq!(after.get(b"a"), Ok(None));
    }

    #[test]
    fn a_scan_stops_at_its_limit_from_either_end() {
        let store = store(&[b"a", b"b", b"c"]);

        assert_eq!(keys(&store, b"", None, Order::Ascending), [b"a", b"b"]);
        assert_eq!(keys(&store, b"", None, Order::Descending), [b"c", b"b"]);
    }

    #[test]
    fn a_scan_includes_its_start_and_not_its_end() {
        let store = store(&[b"a", b"b", b"c"]);

        assert_eq!(keys(&store, b"b", Some(b"c"), Order::Ascending), [b"b"]);
        assert_eq!(keys(&store, b"a", Some(b"b"), Order::Descending), [b"a"]);
    }

    #[test]
    fn a_scan_of_a_backwards_range_is_empty() {
        let store = store(&[b"a", b"b", b"c"]);

        assert_eq!(keys(&store, b"c", Some(b"a"), Order::Ascending), NOTHING);
        assert_eq!(keys(&store, b"b", Some(b"b"), Order::Ascending), NOTHING);
    }
}
