//! Snapshots: model state for a peer that has fallen behind our floor. The
//! transactions it needs are gone, so it gets their result instead.
//!
//! Model data isn't all in memory (see `models`), so both directions go
//! through storage:
//!
//! - **Sending.** A snapshot is a scan of the agreement's data range. Its
//!   floor and Lamport counter are taken when the scan is asked for. Writes
//!   come before reads, so the scan reflects at least that floor. Until the
//!   scan is back, the peer gets the previous snapshot, or nothing.
//! - **Receiving.** Merging is last-writer-wins key by key, so every key in
//!   the snapshot is loaded first. Until then the snapshot waits, much like
//!   a transaction does.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::history::History;
use super::models::Models;
use crate::agreement::AgreementId;
use crate::clock::VersionVector;
use crate::encoding;
use crate::model::{Entry, KvModel, ModelId};
use crate::node::Outbox;
use crate::storage::keys::{self, Record};

/// The state of every model, for a peer that has fallen behind our floor.
///
/// It reflects every transaction we'd integrated when it was taken, not
/// just those below the floor. That's fine: the transactions above the
/// floor travel with it, and applying a last-writer-wins write twice
/// changes nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub(crate) floor: VersionVector,
    pub(crate) lamport: u64,
    pub(crate) models: BTreeMap<ModelId, KvModel>,
}

#[derive(Debug, Default)]
pub(crate) struct Snapshots {
    /// Our latest snapshot, ready to send.
    prepared: Option<Snapshot>,
    /// The floor and Lamport counter of the scan in progress, if any.
    scanning: Option<(VersionVector, u64)>,
    /// Snapshots from peers, waiting for their keys to load.
    arriving: Vec<Snapshot>,
}

impl Snapshots {
    /// Makes sure a snapshot at our current floor is ready or on its way.
    pub(crate) fn prepare(
        &mut self,
        agreement: AgreementId,
        history: &History,
        outbox: &mut Outbox,
    ) {
        let floor = history.floor();
        let current = self.prepared.as_ref().is_some_and(|it| &it.floor == floor);
        if current || self.scanning.is_some() {
            return;
        }

        self.scanning = Some((floor.clone(), history.clock().lamport));
        outbox.scan(keys::data(agreement));
    }

    pub(crate) fn prepared(&self) -> Option<&Snapshot> {
        self.prepared.as_ref()
    }

    /// Our scan of the data range is back.
    pub(crate) fn scanned(&mut self, entries: &[(Vec<u8>, Vec<u8>)]) {
        let Some((floor, lamport)) = self.scanning.take() else {
            return;
        };

        let mut models: BTreeMap<ModelId, KvModel> = BTreeMap::new();
        for (key, value) in entries {
            if let (Some(Record::Entry { model, key, .. }), Some(entry)) =
                (keys::parse(key), encoding::decode::<Entry>(value))
            {
                models.entry(model).or_default().restore(key, entry);
            }
        }

        self.prepared = Some(Snapshot {
            floor,
            lamport,
            models,
        });
    }

    /// A peer sent a snapshot. It waits until every key in it is loaded.
    pub(crate) fn arrived(
        &mut self,
        snapshot: Snapshot,
        agreement: AgreementId,
        models: &mut Models,
        outbox: &mut Outbox,
    ) {
        for (&model, theirs) in &snapshot.models {
            models.request(agreement, model, theirs.keys(), outbox);
        }

        self.arriving.push(snapshot);
    }

    /// Takes the arrived snapshots that can be merged now.
    pub(crate) fn take_loaded(&mut self, models: &Models) -> Vec<Snapshot> {
        let loaded = |snapshot: &Snapshot| {
            (snapshot.models.iter()).all(|(&model, theirs)| models.has(model, theirs.keys()))
        };

        let (ready, waiting) = std::mem::take(&mut self.arriving)
            .into_iter()
            .partition(loaded);
        self.arriving = waiting;

        ready
    }
}
