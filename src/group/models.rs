//! The model entries this device has loaded, and the loads on their way.
//!
//! Hydration doesn't load model data. An entry is loaded the first time
//! something needs to change it, and stays loaded after that.
//!
//! # Why changes wait for loads
//!
//! Last-writer-wins can't apply a write blind: it has to compare the
//! write's stamp with the current entry's. So every change is
//! read-modify-write, and in a state machine that never performs I/O, a read
//! is a round trip through the application. A transaction or snapshot whose
//! keys aren't loaded yet waits for a later batch, the same way a
//! transaction with a gap before it does.
//!
//! The cache stays in step with storage because nothing changes an entry
//! until it's loaded, and storage answers reads after the writes before
//! them (see the storage contract). A loaded entry is never overwritten by
//! a late answer.
//!
//! Nothing is ever evicted. That's the next step for a device with a lot of
//! data: evicting an entry is safe whenever no change is waiting on it.

use std::collections::{BTreeMap, BTreeSet};

use crate::agreement::AgreementId;
use crate::clock::Stamp;
use crate::model::{Entry, KvModel, ModelId, Operation};
use crate::node::Outbox;
use crate::storage::keys;

#[derive(Debug, Default)]
pub(crate) struct Models {
    /// Every loaded entry that exists. Loaded keys with nothing stored are
    /// only in `loaded`.
    entries: BTreeMap<ModelId, KvModel>,
    /// Keys whose current entry is known, whether or not one exists.
    loaded: BTreeSet<(ModelId, Vec<u8>)>,
    /// Keys asked for and not answered yet, so they're only asked for once.
    requested: BTreeSet<(ModelId, Vec<u8>)>,
}

impl Models {
    /// Whether every key is loaded, so a change to them can be applied now.
    pub(crate) fn has<'a>(&self, model: ModelId, mut keys: impl Iterator<Item = &'a [u8]>) -> bool {
        keys.all(|key| self.loaded.contains(&(model, key.to_vec())))
    }

    /// Asks storage for whichever of these keys aren't loaded or on their
    /// way.
    pub(crate) fn request<'a>(
        &mut self,
        agreement: AgreementId,
        model: ModelId,
        keys: impl Iterator<Item = &'a [u8]>,
        outbox: &mut Outbox,
    ) {
        for key in keys {
            let id = (model, key.to_vec());
            if !self.loaded.contains(&id) && self.requested.insert(id) {
                outbox.get(keys::entry(agreement, model, key));
            }
        }
    }

    /// Storage answered for one key.
    pub(crate) fn loaded(&mut self, model: ModelId, key: Vec<u8>, entry: Option<Entry>) {
        let id = (model, key);
        self.requested.remove(&id);
        if !self.loaded.insert(id.clone()) {
            return;
        }

        if let Some(entry) = entry {
            self.entries.entry(model).or_default().restore(id.1, entry);
        }
    }

    /// Applies a transaction's operations. Every key must be loaded.
    pub(crate) fn apply(
        &mut self,
        model: ModelId,
        stamp: Stamp,
        operations: &[Operation],
    ) -> Vec<Vec<u8>> {
        debug_assert!(self.has(model, operations.iter().map(Operation::key)));
        self.entries
            .entry(model)
            .or_default()
            .apply(stamp, operations)
    }

    /// Merges a peer's model state. Every key must be loaded.
    pub(crate) fn merge(&mut self, model: ModelId, theirs: &KvModel) -> Vec<Vec<u8>> {
        debug_assert!(self.has(model, theirs.keys()));
        self.entries.entry(model).or_default().merge(theirs)
    }

    pub(crate) fn entry(&self, model: ModelId, key: &[u8]) -> Option<&Entry> {
        self.entries.get(&model)?.entry(key)
    }

    /// Every loaded entry that exists, for checking against storage.
    #[cfg(test)]
    pub(crate) fn cached(&self) -> impl Iterator<Item = (ModelId, &[u8], &Entry)> {
        self.entries.iter().flat_map(|(&model, kv)| {
            kv.keys()
                .filter_map(move |key| Some((model, key, kv.entry(key)?)))
        })
    }
}
