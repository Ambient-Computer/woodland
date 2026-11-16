//! A view of the store as readers are meant to see it: without any of
//! a checkpoint that's only partly in place.
//!
//! While a group is [veiled], what each key held before is kept beside
//! it. This reads a key from there first, and from where it is
//! otherwise.
//!
//! [veiled]: super::group

use std::collections::BTreeMap;

use crate::encoding;
use crate::store::{Entry, Order, StoreError, View};

use super::group::Shown;
use super::keys::{self, Layer};

#[derive(Clone, Debug)]
pub(crate) struct Veiled<V> {
    view: V,
    shown: Option<Shown>,
}

impl<V: View> Veiled<V> {
    pub(crate) fn new(view: V, shown: Option<Shown>) -> Self {
        Self { view, shown }
    }

    /// What's being looked through, and which part of the group `key`
    /// is in, if it's a part that's veiled.
    fn veils(&self, key: &[u8]) -> Option<(&Shown, Layer)> {
        let shown = self.shown.as_ref()?;
        let layer = keys::layer(key.strip_prefix(shown.prefix.as_slice())?)?;

        Some((shown, layer))
    }
}

impl Shown {
    /// Where what `key` held is kept. A key past the group is past
    /// everything that's kept.
    fn beside(&self, key: &[u8]) -> Vec<u8> {
        match key.strip_prefix(self.prefix.as_slice()) {
            Some(rest) => [&self.before[..], rest].concat(),
            None => keys::range(self.before.clone()).1,
        }
    }

    /// The key that something kept at `beside` was held by.
    fn held_by(&self, beside: &[u8]) -> Vec<u8> {
        let rest = beside.get(self.before.len()..).unwrap_or_default();

        [&self.prefix[..], rest].concat()
    }
}

impl<V: View> View for Veiled<V> {
    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        let Some((shown, layer)) = self.veils(key) else {
            return self.view.get(key).await;
        };

        if shown.blank && layer == Layer::Committed {
            return Ok(None);
        }

        match self.view.get(&shown.beside(key)).await? {
            Some(kept) => Ok(encoding::decode::<Option<Vec<u8>>>(&kept).flatten()),
            None => self.view.get(key).await,
        }
    }

    async fn scan(
        &self,
        start: &[u8],
        end: Option<&[u8]>,
        order: Order,
        limit: usize,
    ) -> Result<Vec<Entry>, StoreError> {
        let Some((shown, layer)) = self.veils(start) else {
            return self.view.scan(start, end, order, limit).await;
        };

        if shown.blank && layer == Layer::Committed {
            return Ok(Vec::new());
        }

        // What's stored and what's kept are each read a page at a
        // time, and neither is known past the end of its page. Each
        // round settles as far as the shorter page goes.
        let ascending = matches!(order, Order::Ascending);
        let mut found = Vec::new();
        let mut from = start.to_vec();
        let mut to = end.map_or_else(|| keys::range(shown.prefix.clone()).1, <[u8]>::to_vec);
        while found.len() < limit && from < to {
            let size = limit - found.len();
            let stored = self.view.scan(&from, Some(&to), order, size).await?;
            let (kept_from, kept_to) = (shown.beside(&from), shown.beside(&to));
            let kept = self.view.scan(&kept_from, Some(&kept_to), order, size);
            let kept = kept.await?;

            let stored_end = stored.last().filter(|_| stored.len() == size);
            let stored_end = stored_end.map(|(key, _)| key.clone());
            let kept_end = kept.last().filter(|_| kept.len() == size);
            let kept_end = kept_end.map(|(key, _)| shown.held_by(key));
            let known = match (stored_end, kept_end) {
                (Some(stored), Some(kept)) if ascending => Some(stored.min(kept)),
                (Some(stored), Some(kept)) => Some(stored.max(kept)),
                (stored, kept) => stored.or(kept),
            };
            let within = |key: &[u8]| match &known {
                Some(known) if ascending => key <= known.as_slice(),
                Some(known) => key >= known.as_slice(),
                None => true,
            };

            let mut settled: BTreeMap<_, _> = stored.into_iter().collect();
            settled.retain(|key, _| within(key));
            for (beside, was) in kept {
                let key = shown.held_by(&beside);
                if !within(&key) {
                    continue;
                }

                match encoding::decode::<Option<Vec<u8>>>(&was).flatten() {
                    Some(value) => settled.insert(key, value),
                    None => settled.remove(&key),
                };
            }

            if ascending {
                found.extend(settled);
            } else {
                found.extend(settled.into_iter().rev());
            }

            match known {
                Some(known) if ascending => from = keys::after(&known),
                Some(known) => to = known,
                None => break,
            }
        }

        found.truncate(limit);

        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::db::GroupId;
    use crate::model::ModelId;
    use crate::store::{MemoryStore, Store, Write};
    use crate::testing::block_on;

    fn group() -> GroupId {
        GroupId::from_bytes([1; 32])
    }

    fn key(name: u8) -> Vec<u8> {
        let model = ModelId::kv(group(), [2; 32]);

        keys::committed_value(group(), model, &[b'k', name])
    }

    fn shown() -> Shown {
        Shown {
            prefix: keys::group(group()),
            before: keys::before(group()),
            blank: false,
        }
    }

    proptest! {
        /// Whatever is stored and whatever was kept, a reader finds
        /// each key as it was kept, or as it's stored if it wasn't:
        /// one at a time, or any range of them, either way round, any
        /// number at a time.
        #[test]
        fn a_reader_finds_what_was_kept_first(
            stored in prop::collection::btree_map(0..24u8, any::<u8>(), 0..20),
            kept in prop::collection::btree_map(0..24u8, prop::option::of(any::<u8>()), 0..20),
            (from, to) in (0..26u8, 0..26u8),
            ascending in any::<bool>(),
            limit in 1..30usize,
        ) {
            let shown = shown();
            let mut batch = Vec::new();
            for (&name, &value) in &stored {
                batch.push(Write::Put { key: key(name), value: vec![value] });
            }
            for (&name, &was) in &kept {
                let was = was.map(|was| vec![was]);
                batch.push(Write::Put {
                    key: shown.beside(&key(name)),
                    value: encoding::encode(&was),
                });
            }
            let mut store = MemoryStore::new();
            block_on(store.apply(batch)).expect("stored");
            let view = Veiled::new(store.view(), Some(shown));

            let mut expected: BTreeMap<u8, u8> = stored.clone();
            for (&name, &was) in &kept {
                match was {
                    Some(was) => expected.insert(name, was),
                    None => expected.remove(&name),
                };
            }

            for name in 0..24 {
                let found = block_on(view.get(&key(name))).expect("read");
                prop_assert_eq!(found, expected.get(&name).map(|&value| vec![value]));
            }

            let order = if ascending { Order::Ascending } else { Order::Descending };
            let within = expected.range(from.min(to)..from.max(to));
            let mut within: Vec<_> = within.map(|(&name, &value)| (key(name), vec![value])).collect();
            if !ascending {
                within.reverse();
            }
            within.truncate(limit);
            let (start, end) = (key(from.min(to)), key(from.max(to)));
            let found = block_on(view.scan(&start, Some(&end), order, limit)).expect("scanned");
            prop_assert_eq!(found, within);
        }
    }
}
