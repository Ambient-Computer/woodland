//! Snapshots: model state for a peer that has fallen behind our floor. The
//! transactions it needs are gone, so it gets their result instead.
//!
//! Model data is never in memory, so sending goes through storage. A
//! snapshot is a scan of the agreement's data range. Its floor and Lamport
//! counter are taken when the scan is asked for. Writes come before reads,
//! so the scan reflects at least that floor. Until the scan is back, the
//! peer gets the previous snapshot, or nothing.
//!
//! Receiving doesn't read anything. Versions merge by union, so a
//! snapshot's versions are written like a transaction's are.

use serde::{Deserialize, Serialize};

use super::history::History;
use crate::agreement::AgreementId;
use crate::clock::{Stamp, VersionVector};
use crate::encoding;
use crate::model::{ModelId, Version};
use crate::node::Outbox;
use crate::storage::keys::{self, Record};

/// The state of every model, for a peer that has fallen behind our floor.
///
/// It reflects every transaction we'd integrated when it was taken, not
/// just those below the floor. That's fine: the transactions above the
/// floor travel with it, and writing a version twice changes nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub(crate) floor: VersionVector,
    pub(crate) lamport: u64,
    pub(crate) versions: Vec<Versioned>,
}

/// One version of one key, as a snapshot carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Versioned {
    pub(crate) model: ModelId,
    pub(crate) key: Vec<u8>,
    pub(crate) stamp: Stamp,
    pub(crate) version: Version,
}

#[derive(Debug, Default)]
pub(crate) struct Snapshots {
    /// Our latest snapshot, ready to send.
    prepared: Option<Snapshot>,
    /// The floor and Lamport counter of the scan in progress, if any.
    scanning: Option<(VersionVector, u64)>,
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

        let versions = entries.iter().filter_map(|(key, value)| {
            let Record::Version {
                model, key, stamp, ..
            } = keys::parse(key)?
            else {
                return None;
            };

            Some(Versioned {
                model,
                key,
                stamp: *stamp,
                version: encoding::decode(value)?,
            })
        });

        self.prepared = Some(Snapshot {
            floor,
            lamport,
            versions: versions.collect(),
        });
    }
}
