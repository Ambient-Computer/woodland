use std::collections::BTreeMap;
use std::future::ready;
use std::ops::Bound;
use std::sync::Arc;

use super::{Entry, Order, Store, StoreError, View, Write};

type Entries = BTreeMap<Vec<u8>, Vec<u8>>;

/// The whole contract in a map in memory, for tests and simulations.
///
/// It answers at once, so nothing built on it ever waits. And it keeps
/// nothing when the process ends, so a device that uses it for real is a
/// new device every time it starts.
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
    type View = MemoryView;

    fn apply(&mut self, batch: Vec<Write>) -> impl Future<Output = Result<(), StoreError>> {
        let entries = Arc::make_mut(&mut self.entries);
        for write in batch {
            match write {
                Write::Put { key, value } => entries.insert(key, value),
                Write::Delete { key } => entries.remove(&key),
            };
        }

        ready(Ok(()))
    }

    fn view(&self) -> MemoryView {
        MemoryView(Arc::clone(&self.entries))
    }
}

/// A [`MemoryStore`] at one moment.
#[derive(Clone, Debug)]
pub struct MemoryView(Arc<Entries>);

impl MemoryView {
    fn range(&self, start: &[u8], end: Option<&[u8]>, order: Order, limit: usize) -> Vec<Entry> {
        if end.is_some_and(|end| end <= start) {
            return Vec::new();
        }

        let end = end.map_or(Bound::Unbounded, Bound::Excluded);
        let range = self.0.range::<[u8], _>((Bound::Included(start), end));
        let owned = |(key, value): (&Vec<u8>, &Vec<u8>)| (key.clone(), value.clone());

        match order {
            Order::Ascending => range.take(limit).map(owned).collect(),
            Order::Descending => range.rev().take(limit).map(owned).collect(),
        }
    }
}

impl View for MemoryView {
    fn get(&self, key: &[u8]) -> impl Future<Output = Result<Option<Vec<u8>>, StoreError>> {
        ready(Ok(self.0.get(key).cloned()))
    }

    fn scan(
        &self,
        start: &[u8],
        end: Option<&[u8]>,
        order: Order,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<Entry>, StoreError>> {
        ready(Ok(self.range(start, end, order, limit)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::block_on;

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

    fn apply(store: &mut MemoryStore, batch: Vec<Write>) {
        block_on(store.apply(batch)).expect("memory doesn't fail");
    }

    fn get(view: &MemoryView, key: &[u8]) -> Option<Vec<u8>> {
        block_on(view.get(key)).expect("memory doesn't fail")
    }

    fn store(keys: &[&[u8]]) -> MemoryStore {
        let mut store = MemoryStore::new();
        apply(&mut store, keys.iter().map(|key| put(key, key)).collect());

        store
    }

    fn keys(store: &MemoryStore, start: &[u8], end: Option<&[u8]>, order: Order) -> Vec<Vec<u8>> {
        let entries = block_on(store.view().scan(start, end, order, 2));
        let entries = entries.expect("memory doesn't fail");

        entries.into_iter().map(|(key, _)| key).collect()
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

        apply(&mut store, batch);

        let view = store.view();
        assert_eq!(get(&view, b"a"), Some(b"2".to_vec()));
        assert_eq!(get(&view, b"b"), None);
    }

    #[test]
    fn a_view_doesnt_see_what_came_after_it() {
        let mut store = store(&[b"a"]);
        let before = store.view();

        apply(&mut store, vec![delete(b"a"), put(b"b", b"b")]);

        assert_eq!(get(&before, b"a"), Some(b"a".to_vec()));
        assert_eq!(get(&before, b"b"), None);
        assert_eq!(get(&store.view(), b"a"), None);
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
