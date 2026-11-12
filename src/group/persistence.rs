//! How a group maps onto storage, and how it's rebuilt from it (hydration).
//!
//! What's stored, under the agreement's prefix (see [`keys`]):
//!
//! - the **genesis**, so the agreement ID can always be re-derived;
//! - the agreement **view**, so learned decisions survive restarts, and
//!   this device's latest **attestation** of it, so revisions keep rising;
//! - the **clock** (integrated version vector, Lamport counter, floor);
//! - the **log** of signed transactions above the floor, plus this device's
//!   own transactions still waiting for write access;
//! - every **version** of every model key, one storage key each.
//!
//! What isn't: remote transactions still waiting, and everything about
//! peers. Peers re-send whatever we report missing when they reconnect.
//!
//! Hydration loads everything except model data: the protocol range of the
//! key layout. Its size is bounded by what hasn't been compacted yet, not
//! by how much data the models hold. The node only ever writes model data.
//! Reading it is the application's business (see [`Query`]).
//!
//! [`Query`]: crate::Query

use std::collections::BTreeMap;

use super::Group;
use super::snapshot::Snapshots;
use crate::actor::{Peer, Signed, Verified};
use crate::agreement::{Agreement, AgreementId, Attestation, Genesis, View};
use crate::clock::Stamp;
use crate::encoding;
use crate::log::Transaction;
use crate::log::{Clock, Log};
use crate::model::{ModelId, Operation};
use crate::node::Outbox;
use crate::storage::keys::{self, Record};

/// Everything needed to rebuild a group, gathered from storage.
#[derive(Debug)]
pub(crate) struct Restored {
    genesis: Genesis,
    me: Peer,
    view: Option<View>,
    attested: Option<Verified<Attestation>>,
    clock: Clock,
    transactions: Vec<Verified<Transaction>>,
}

/// The last view, attestation and clock written, so unchanged ones aren't
/// rewritten.
#[derive(Debug, Default)]
pub(super) struct Persisted {
    view: Option<View>,
    /// The revision of the attestation written.
    attested: Option<u64>,
    clock: Option<Clock>,
}

impl Restored {
    pub(crate) fn fresh(genesis: &Genesis, me: Peer) -> Self {
        Self {
            genesis: genesis.clone(),
            me,
            view: None,
            attested: None,
            clock: Clock::default(),
            transactions: Vec::new(),
        }
    }

    /// Reads a scan of the agreement's protocol range. Entries that don't
    /// decode are skipped: they can only come from a bug or a corrupted
    /// store.
    pub(crate) fn from_entries(
        genesis: &Genesis,
        me: Peer,
        entries: &[(Vec<u8>, Vec<u8>)],
    ) -> Self {
        let mut restored = Self::fresh(genesis, me);
        for (key, value) in entries {
            match keys::parse(key) {
                Some(Record::View(_)) => restored.view = encoding::decode(value),
                Some(Record::Attestation(_)) => {
                    // Only this device wrote it, and it signed it.
                    let signed = encoding::decode::<Signed<Attestation>>(value);
                    restored.attested = signed.map(Verified::trust_stored);
                }
                Some(Record::Clock(_)) => {
                    restored.clock = encoding::decode(value).unwrap_or_default();
                }
                Some(Record::Log { .. }) => {
                    // Only this device wrote it, after verifying it.
                    if let Some(signed) = encoding::decode::<Signed<Transaction>>(value) {
                        restored.transactions.push(Verified::trust_stored(signed));
                    }
                }
                Some(Record::Genesis(_) | Record::Version { .. }) | None => {}
            }
        }

        restored
    }
}

impl From<Restored> for Group {
    fn from(restored: Restored) -> Self {
        let id = restored.genesis.id();
        let attested = restored.attested.as_ref().map(|it| it.payload().revision);
        let agreement = match restored.view.clone() {
            Some(view) => Agreement::resume(id, view, restored.attested, restored.me),
            None => Agreement::new(&restored.genesis, restored.me),
        };

        Self {
            id,
            me: restored.me,
            agreement,
            log: Log::restore(restored.clock.clone(), restored.transactions, restored.me),
            snapshots: Snapshots::default(),
            peers: BTreeMap::new(),
            persisted: Persisted {
                view: restored.view,
                attested,
                clock: Some(restored.clock),
            },
        }
    }
}

impl Persisted {
    /// Writes the view, attestation and clock, if they changed since the
    /// last write.
    pub(super) fn save(
        &mut self,
        id: AgreementId,
        agreement: &Agreement,
        log: &Log,
        outbox: &mut Outbox,
    ) {
        if self.view.as_ref() != Some(agreement.view()) {
            outbox.put(keys::view(id), agreement.view());
            self.view = Some(agreement.view().clone());
        }

        if let Some(attested) = agreement.attested()
            && self.attested != Some(attested.payload().revision)
        {
            outbox.put(keys::attestation(id), attested.signed());
            self.attested = Some(attested.payload().revision);
        }

        if self.clock.as_ref() != Some(log.clock()) {
            outbox.put(keys::clock(id), log.clock());
            self.clock = Some(log.clock().clone());
        }
    }
}

pub(super) fn save_genesis(genesis: &Genesis, outbox: &mut Outbox) {
    outbox.put(keys::genesis(genesis.id()), genesis);
}

pub(super) fn save_transaction(id: AgreementId, signed: &Signed<Transaction>, outbox: &mut Outbox) {
    let key = keys::log(id, signed.author(), signed.payload().seq);
    outbox.put(key, signed);
}

/// Writes what a transaction's operations leave behind, in order, so a
/// later operation on the same key replaces an earlier one.
pub(super) fn save_operations(
    id: AgreementId,
    model: ModelId,
    stamp: Stamp,
    operations: &[Operation],
    outbox: &mut Outbox,
) {
    for operation in operations {
        let write = operation.write();
        save_version(id, model, &write.key, stamp, write.value, outbox);
    }
}

/// Writes one version as the model encoded it.
pub(super) fn save_version(
    id: AgreementId,
    model: ModelId,
    key: &[u8],
    stamp: Stamp,
    value: Vec<u8>,
    outbox: &mut Outbox,
) {
    outbox.put_bytes(keys::version(id, model, key, stamp), value);
}

pub(super) fn forget_transactions(
    id: AgreementId,
    author: Peer,
    through: u64,
    outbox: &mut Outbox,
) {
    let (start, end) = keys::log_through(id, author, through);
    outbox.delete_range(start, end);
}
