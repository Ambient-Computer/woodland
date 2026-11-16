//! One step: everything a batch of work reads, writes and says.
//!
//! A step reads the store as it was when the step began, plus whatever
//! the step itself has written since. Its writes are applied together
//! when it ends, and only then is anything it wanted to say let out.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::actor::Peer;
use crate::db::{Changed, keys};
use crate::encoding;
use crate::store::{Entry, Order, StoreError, View, Write};

/// How many entries to read from the store at once.
const PAGE: usize = 256;

#[derive(Debug)]
pub(crate) struct Step<V> {
    view: V,
    /// What this step has written, or deleted, so far.
    pending: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    sends: Vec<(Peer, Vec<u8>)>,
    changed: BTreeSet<Changed>,
    /// A prefix, and where writes to keys that start with it are kept
    /// instead: see [`divert`](Self::divert).
    aside: Option<(Vec<u8>, Vec<u8>)>,
}

/// How something kept to one side starts: it's a value, or it's the
/// lack of one.
const PUT: u8 = 1;
const DELETE: u8 = 0;

/// What something kept to one side says was written.
fn written(kept: &[u8]) -> Option<Vec<u8>> {
    match kept.split_first() {
        Some((&PUT, value)) => Some(value.to_vec()),
        _ => None,
    }
}

impl<V: View> Step<V> {
    pub(crate) fn new(view: V) -> Self {
        Self {
            view,
            pending: BTreeMap::new(),
            sends: Vec::new(),
            changed: BTreeSet::new(),
            aside: None,
        }
    }

    /// From here on, whatever's written to a key that starts with
    /// `prefix` is kept under `aside` instead, and read back from
    /// there as if it had been written. Nothing is sent and nothing is
    /// noticed. `None` ends that.
    pub(crate) fn divert(&mut self, aside: Option<(Vec<u8>, Vec<u8>)>) {
        self.aside = aside;
    }

    /// Where a write to `key` is kept, if it's being kept to one side.
    fn beside(&self, key: &[u8]) -> Option<Vec<u8>> {
        let (prefix, aside) = self.aside.as_ref()?;
        let rest = key.strip_prefix(prefix.as_slice())?;

        Some([aside.as_slice(), rest].concat())
    }

    /// The key that something kept to one side at `beside` was written
    /// to.
    fn written_to(&self, beside: &[u8]) -> Option<Vec<u8>> {
        let (prefix, aside) = self.aside.as_ref()?;
        let rest = beside.strip_prefix(aside.as_slice())?;

        Some([prefix.as_slice(), rest].concat())
    }

    pub(crate) fn put(&mut self, key: Vec<u8>, value: Vec<u8>) {
        match self.beside(&key) {
            Some(beside) => self
                .pending
                .insert(beside, Some([&[PUT][..], &value].concat())),
            None => self.pending.insert(key, Some(value)),
        };
    }

    pub(crate) fn save(&mut self, key: Vec<u8>, value: &impl Serialize) {
        self.put(key, encoding::encode(value));
    }

    pub(crate) fn delete(&mut self, key: Vec<u8>) {
        match self.beside(&key) {
            Some(beside) => self.pending.insert(beside, Some(vec![DELETE])),
            None => self.pending.insert(key, None),
        };
    }

    pub(crate) fn send(&mut self, peer: Peer, message: &impl Serialize) {
        if self.aside.is_none() {
            self.sends.push((peer, encoding::encode(message)));
        }
    }

    pub(crate) fn changed(&mut self, change: Changed) {
        if self.aside.is_none() {
            self.changed.insert(change);
        }
    }

    async fn held(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        match self.pending.get(key) {
            Some(written) => Ok(written.clone()),
            None => self.view.get(key).await,
        }
    }

    pub(crate) async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        if let Some(beside) = self.beside(key)
            && let Some(kept) = self.held(&beside).await?
        {
            return Ok(written(&kept));
        }

        self.held(key).await
    }

    /// Reads back something this library stored. If it doesn't decode,
    /// it's as good as missing.
    pub(crate) async fn load<T: DeserializeOwned>(
        &self,
        key: &[u8],
    ) -> Result<Option<T>, StoreError> {
        let stored = self.get(key).await?;

        Ok(stored.and_then(|bytes| encoding::decode(&bytes)))
    }

    /// Everything with a key in `start..end`, in key order.
    pub(crate) async fn scan(&self, start: &[u8], end: &[u8]) -> Result<Vec<Entry>, StoreError> {
        let mut found = self.all(start, end).await?;
        if let (Some(from), Some(to)) = (self.beside(start), self.beside(end)) {
            for (beside, kept) in self.all(&from, &to).await? {
                let Some(key) = self.written_to(&beside) else {
                    continue;
                };

                match written(&kept) {
                    Some(value) => found.insert(key, value),
                    None => found.remove(&key),
                };
            }
        }

        Ok(found.into_iter().collect())
    }

    async fn all(
        &self,
        start: &[u8],
        end: &[u8],
    ) -> Result<BTreeMap<Vec<u8>, Vec<u8>>, StoreError> {
        let mut found: BTreeMap<_, _> = scan(&self.view, start, end).await?.into_iter().collect();
        for (key, written) in self.pending.range(start.to_vec()..end.to_vec()) {
            match written {
                Some(value) => found.insert(key.clone(), value.clone()),
                None => found.remove(key),
            };
        }

        Ok(found)
    }

    /// The first `limit` things with a key in `start..end`, in key
    /// order. Fewer only if that's all there are.
    pub(crate) async fn scan_page(
        &self,
        start: &[u8],
        end: &[u8],
        limit: usize,
    ) -> Result<Vec<Entry>, StoreError> {
        if self.beside(start).is_none() {
            return self.some(start, end, limit).await;
        }

        // What's stored and what's kept to one side are each read a
        // page at a time, and neither is known past the end of its
        // page.
        let mut found = BTreeMap::new();
        let mut from = start.to_vec();
        while found.len() < limit {
            let (Some(beside), Some(to)) = (self.beside(&from), self.beside(end)) else {
                break;
            };
            let stored = self.some(&from, end, limit).await?;
            let kept = self.some(&beside, &to, limit).await?;
            let reach = |page: &[Entry]| {
                let last = page.last().filter(|_| page.len() == limit);

                last.map(|(key, _)| keys::after(key))
            };
            let stored_to = reach(&stored).unwrap_or_else(|| end.to_vec());
            let kept_to = reach(&kept).and_then(|key| self.written_to(&key));
            let known = stored_to.min(kept_to.unwrap_or_else(|| end.to_vec()));

            found.extend(stored.into_iter().filter(|(key, _)| *key < known));
            for (beside, kept) in kept {
                let Some(key) = self.written_to(&beside).filter(|key| *key < known) else {
                    continue;
                };

                match written(&kept) {
                    Some(value) => found.insert(key, value),
                    None => found.remove(&key),
                };
            }
            if known.as_slice() >= end {
                break;
            }
            from = known;
        }

        Ok(found.into_iter().take(limit).collect())
    }

    async fn some(&self, start: &[u8], end: &[u8], limit: usize) -> Result<Vec<Entry>, StoreError> {
        let mut found = BTreeMap::new();
        let mut from = start.to_vec();
        while found.len() < limit {
            let stored = self.view.scan(&from, Some(end), Order::Ascending, limit);
            let stored = stored.await?;

            // Past the last key read, what's stored is unknown, so
            // what's pending there has to wait. What this step deleted
            // may leave the page short with more beyond it.
            let last = stored.last().filter(|_| stored.len() == limit);
            let known = last.map_or_else(|| end.to_vec(), |(key, _)| keys::after(key));
            found.extend(stored);
            for (key, written) in self.pending.range(from..known.clone()) {
                match written {
                    Some(value) => found.insert(key.clone(), value.clone()),
                    None => found.remove(key),
                };
            }
            if known.as_slice() >= end {
                break;
            }
            from = known;
        }

        Ok(found.into_iter().take(limit).collect())
    }

    /// The step's writes as one batch, and what to let out once it's
    /// durable.
    pub(crate) fn finish(self) -> Finished {
        let write = |(key, written)| match written {
            Some(value) => Write::Put { key, value },
            None => Write::Delete { key },
        };

        Finished {
            batch: self.pending.into_iter().map(write).collect(),
            sends: self.sends,
            changed: self.changed.into_iter().collect(),
        }
    }
}

/// What a step came to.
#[derive(Debug)]
pub(crate) struct Finished {
    pub(crate) batch: Vec<Write>,
    pub(crate) sends: Vec<(Peer, Vec<u8>)>,
    pub(crate) changed: Vec<Changed>,
}

/// Everything `view` holds with a key in `start..end`, in key order.
pub(crate) async fn scan<V: View>(
    view: &V,
    start: &[u8],
    end: &[u8],
) -> Result<Vec<Entry>, StoreError> {
    let mut found = Vec::new();
    let mut from = start.to_vec();
    loop {
        let page = view.scan(&from, Some(end), Order::Ascending, PAGE).await?;
        let more = page.len() == PAGE;
        if let Some((last, _)) = page.last() {
            from = keys::after(last);
        }
        found.extend(page);
        if !more {
            return Ok(found);
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::store::{MemoryStore, Store};
    use crate::testing::block_on;

    type Wrote = (u8, Option<u8>);

    fn key(name: u8) -> Vec<u8> {
        vec![b'g', name % 2, name]
    }

    fn apply(step: &mut Step<<MemoryStore as Store>::View>, writes: &[Wrote]) {
        for &(name, value) in writes {
            match value {
                Some(value) => step.put(key(name), vec![value]),
                None => step.delete(key(name)),
            }
        }
    }

    fn model(all: &[&[Wrote]]) -> BTreeMap<Vec<u8>, Vec<u8>> {
        let mut model = BTreeMap::new();
        for &(name, value) in all.iter().copied().flatten() {
            match value {
                Some(value) => model.insert(key(name), vec![value]),
                None => model.remove(&key(name)),
            };
        }

        model
    }

    fn writes() -> impl Strategy<Value = Vec<Wrote>> {
        prop::collection::vec((0..24u8, prop::option::of(any::<u8>())), 0..40)
    }

    proptest! {
        /// Writes kept to one side read back as if they'd been made,
        /// however they're read, and aren't made.
        #[test]
        fn writes_kept_to_one_side_read_back_as_written(
            stored in writes(),
            earlier in writes(),
            before in writes(),
            aside in writes(),
            limit in 1..9usize,
            start in 0..24u8,
        ) {
            let mut store = MemoryStore::new();
            let mut step = Step::new(store.view());
            apply(&mut step, &stored);
            block_on(store.apply(step.finish().batch)).expect("stored");

            // Some of what's kept to one side is from an earlier step.
            let divert = Some((b"g".to_vec(), b"h".to_vec()));
            let mut step = Step::new(store.view());
            step.divert(divert.clone());
            apply(&mut step, &earlier);
            block_on(store.apply(step.finish().batch)).expect("stored");

            let mut step = Step::new(store.view());
            apply(&mut step, &before);
            step.divert(divert);
            apply(&mut step, &aside);

            let expected = model(&[&stored, &before, &earlier, &aside]);
            let (from, end) = (vec![b'g', 0, start], b"g\xFF".to_vec());
            let within: Vec<_> = expected.range(from.clone()..end.clone()).collect();
            let within: Vec<Entry> = within.into_iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            for name in 0..24 {
                let held = block_on(step.get(&key(name))).expect("read");
                prop_assert_eq!(held.as_ref(), expected.get(&key(name)));
            }
            prop_assert_eq!(&block_on(step.scan(&from, &end)).expect("read"), &within);
            let page = block_on(step.scan_page(&from, &end, limit)).expect("read");
            prop_assert_eq!(page, within.iter().take(limit).cloned().collect::<Vec<_>>());

            step.changed(Changed::Everything { group: crate::db::GroupId::from_bytes([1; 32]) });
            step.divert(None);
            let unmoved = model(&[&stored, &before]);
            let seen = block_on(step.scan(b"g", b"g\xFF")).expect("read");
            prop_assert_eq!(seen, unmoved.into_iter().collect::<Vec<_>>());
            prop_assert!(step.finish().changed.is_empty());
        }
    }
}
