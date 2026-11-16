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
    /// How many times each key has been written.
    #[cfg(test)]
    pub(crate) puts: BTreeMap<Vec<u8>, usize>,
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
            #[cfg(test)]
            if let Write::Put { key, .. } = &write {
                *self.puts.entry(key.clone()).or_default() += 1;
            }

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
    use crate::store::contract;
    use crate::testing::block_on;

    #[test]
    fn it_does_what_every_store_does() {
        block_on(contract::holds(|| ready(MemoryStore::new())));
    }
}
