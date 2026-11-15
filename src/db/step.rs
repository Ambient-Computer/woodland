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
}

impl<V: View> Step<V> {
    pub(crate) fn new(view: V) -> Self {
        Self {
            view,
            pending: BTreeMap::new(),
            sends: Vec::new(),
            changed: BTreeSet::new(),
        }
    }

    /// A copy to try things out on. What it writes goes nowhere.
    pub(crate) fn fork(&self) -> Self {
        Self {
            view: self.view.clone(),
            pending: self.pending.clone(),
            sends: Vec::new(),
            changed: BTreeSet::new(),
        }
    }

    pub(crate) fn put(&mut self, key: Vec<u8>, value: Vec<u8>) {
        self.pending.insert(key, Some(value));
    }

    pub(crate) fn save(&mut self, key: Vec<u8>, value: &impl Serialize) {
        self.put(key, encoding::encode(value));
    }

    pub(crate) fn delete(&mut self, key: Vec<u8>) {
        self.pending.insert(key, None);
    }

    pub(crate) fn send(&mut self, peer: Peer, message: &impl Serialize) {
        self.sends.push((peer, encoding::encode(message)));
    }

    pub(crate) fn changed(&mut self, change: Changed) {
        self.changed.insert(change);
    }

    pub(crate) async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        match self.pending.get(key) {
            Some(written) => Ok(written.clone()),
            None => self.view.get(key).await,
        }
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
        let mut found: BTreeMap<_, _> = scan(&self.view, start, end).await?.into_iter().collect();
        for (key, written) in self.pending.range(start.to_vec()..end.to_vec()) {
            match written {
                Some(value) => found.insert(key.clone(), value.clone()),
                None => found.remove(key),
            };
        }

        Ok(found.into_iter().collect())
    }

    /// The first `limit` things with a key in `start..end`, in key order.
    pub(crate) async fn scan_page(
        &self,
        start: &[u8],
        end: &[u8],
        limit: usize,
    ) -> Result<Vec<Entry>, StoreError> {
        let stored = self.view.scan(start, Some(end), Order::Ascending, limit);
        let stored = stored.await?;

        // Past the last key read, what's stored is unknown, so what's
        // pending there has to wait for the next page.
        let last = stored.last().filter(|_| stored.len() == limit);
        let known = last.map_or_else(|| end.to_vec(), |(key, _)| keys::after(key));
        let mut found: BTreeMap<_, _> = stored.into_iter().collect();
        for (key, written) in self.pending.range(start.to_vec()..known) {
            match written {
                Some(value) => found.insert(key.clone(), value.clone()),
                None => found.remove(key),
            };
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
