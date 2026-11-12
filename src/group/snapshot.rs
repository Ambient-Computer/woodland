//! Snapshots: settled state for a peer that has fallen behind our settled
//! cut. The transactions it needs are gone, so it gets their result
//! instead.
//!
//! A snapshot is everything in the settled range of storage: the cut, and
//! one value per key. They're written together, so a scan of that range
//! always holds a consistent state and the cut it belongs to, whenever the
//! scan happens to run. Until the scan is back, the peer gets the previous
//! snapshot, or nothing.
//!
//! Receiving doesn't read anything. Settled states are prefixes of one
//! timeline, so a snapshot that's ahead of ours replaces ours wholesale
//! (see `settling`). What we hold above its cut stays in the frontier.

use serde::{Deserialize, Serialize};

use crate::agreement::AgreementId;
use crate::encoding;
use crate::log::{Cut, Log};
use crate::model::ModelId;
use crate::node::Outbox;
use crate::storage::keys::{self, Record};

/// The settled state of every model, and the cut it reflects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub(crate) cut: Cut,
    pub(crate) values: Vec<SettledValue>,
}

/// One key's settled value, as a snapshot carries it: exactly what the
/// model stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SettledValue {
    pub(crate) model: ModelId,
    pub(crate) key: Vec<u8>,
    pub(crate) value: Vec<u8>,
}

#[derive(Debug, Default)]
pub(crate) struct Snapshots {
    /// Our latest snapshot, ready to send.
    prepared: Option<Snapshot>,
    /// Whether a scan is in progress.
    scanning: bool,
}

impl Snapshots {
    /// Makes sure a snapshot at our current cut is ready or on its way.
    pub(crate) fn prepare(&mut self, agreement: AgreementId, log: &Log, outbox: &mut Outbox) {
        let current = self
            .prepared
            .as_ref()
            .is_some_and(|it| &it.cut == log.cut());
        if current || self.scanning {
            return;
        }

        self.scanning = true;
        outbox.scan(keys::settled(agreement));
    }

    pub(crate) fn prepared(&self) -> Option<&Snapshot> {
        self.prepared.as_ref()
    }

    /// Our scan of the settled range is back.
    pub(crate) fn scanned(&mut self, entries: &[(Vec<u8>, Vec<u8>)]) {
        if !std::mem::take(&mut self.scanning) {
            return;
        }

        let mut cut = None;
        let mut values = Vec::new();
        for (key, value) in entries {
            match keys::parse(key) {
                Some(Record::Cut(_)) => cut = encoding::decode(value),
                Some(Record::Settled { model, key, .. }) => values.push(SettledValue {
                    model,
                    key,
                    value: value.clone(),
                }),
                _ => {}
            }
        }

        // Without a cut, nothing has settled, and there's nothing to send.
        self.prepared = cut.map(|cut| Snapshot { cut, values });
    }
}
