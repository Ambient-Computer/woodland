//! How a group maps onto storage, and how it's rebuilt from it (hydration).
//!
//! What's stored, under the agreement's prefix (see [`keys`]):
//!
//! - the **genesis**, so the agreement ID can always be re-derived;
//! - the agreement **view**, so learned decisions survive restarts, and
//!   this device's latest **attestation** of it, so revisions keep rising;
//! - the **clock** (integrated version vector, Lamport counter, cut);
//! - the **log** of signed transactions that haven't settled;
//! - writes **held** until this device is admitted, unsigned;
//! - the **settled** value of every model key, and the cut they reflect;
//! - a **version** for every write that hasn't settled, of every
//!   transaction that hasn't been pruned.
//!
//! What isn't: remote transactions still waiting, and everything about
//! peers. Peers re-send whatever we report missing when they reconnect.
//!
//! Hydration loads everything except model data: the protocol range of the
//! key layout. Its size is bounded by what hasn't settled yet, not by how
//! much data the models hold. The node only ever writes model data.
//! Reading it is the application's business (see [`Query`]).
//!
//! [`Query`]: crate::Query

use std::collections::BTreeMap;

use super::Group;
use super::admission::{self, Held};
use super::snapshot::{Snapshot, Snapshots};
use super::verdicts::Verdicts;
use crate::actor::{Peer, Signed, Verified};
use crate::agreement::{Agreement, AgreementId, Attestation, Genesis, View};
use crate::clock::Stamp;
use crate::encoding;
use crate::log::Transaction;
use crate::log::{Clock, Log};
use crate::model::{Kind, Operation, Write};
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
    held: Vec<(u64, admission::HeldWrite)>,
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
            held: Vec::new(),
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
                Some(Record::Held { index, .. }) => {
                    if let Some(write) = encoding::decode(value) {
                        restored.held.push((index, write));
                    }
                }
                // Model data isn't in the protocol range, and never loads.
                Some(
                    Record::Genesis(_)
                    | Record::Cut(_)
                    | Record::Settled { .. }
                    | Record::Version { .. },
                )
                | None => {}
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

        // Founders never wait to be admitted (see `admission`).
        let mut log = Log::restore(restored.clock.clone(), restored.transactions, restored.me);
        if restored.genesis.configuration().is_member(restored.me) {
            log.admit();
        }

        Self {
            id,
            me: restored.me,
            agreement,
            verdicts: Verdicts::resume(&log.frontier()),
            log,
            held: Held::restore(restored.held),
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
    /// last write. The cut is written a second time next to the settled
    /// values, in the same write that changes them, so a scan of that
    /// range always holds both.
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
            let cut = self.clock.as_ref().map(|it| &it.cut);
            if cut != Some(log.cut()) {
                outbox.put(keys::cut(id), log.cut());
            }

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

/// Writes what a live transaction's operations leave behind, as versions
/// in the frontier. Not for models that only show what has settled:
/// their writes wait in the log. In order, so a later operation on the same key replaces
/// an earlier one.
pub(super) fn save_operations(
    id: AgreementId,
    stamp: Stamp,
    operations: &[Operation],
    outbox: &mut Outbox,
) {
    let shown = |write: &Write| write.model.kind().is_some_and(Kind::shows_unsettled);
    for write in operations.iter().map(Operation::write).filter(shown) {
        let key = keys::version(id, write.model, &write.key, stamp);
        outbox.put(key, &write.value);
    }
}

/// Makes a live transaction's writes the settled values of their keys.
/// Transactions must be folded in timeline order.
pub(super) fn fold_operations(id: AgreementId, operations: &[Operation], outbox: &mut Outbox) {
    for write in operations.iter().map(Operation::write) {
        let settled = keys::settled_value(id, write.model, &write.key);
        match write.value {
            Some(value) => outbox.put_bytes(settled, value),
            None => outbox.delete(settled),
        }
    }
}

/// Deletes a transaction's versions and its place in the log: it settled,
/// here or in a snapshot.
pub(super) fn forget_transaction(
    id: AgreementId,
    transaction: &Verified<Transaction>,
    outbox: &mut Outbox,
) {
    let (author, payload) = (transaction.author(), transaction.payload());
    forget_operations(id, payload.stamp(author), &payload.operations, outbox);
    outbox.delete(keys::log(id, author, payload.seq));
}

/// Deletes a transaction's versions: it was pruned, or it settled.
pub(super) fn forget_operations(
    id: AgreementId,
    stamp: Stamp,
    operations: &[Operation],
    outbox: &mut Outbox,
) {
    for write in operations.iter().map(Operation::write) {
        outbox.delete(keys::version(id, write.model, &write.key, stamp));
    }
}

/// Replaces everything settled with a snapshot's state.
pub(super) fn save_snapshot(id: AgreementId, snapshot: &Snapshot, outbox: &mut Outbox) {
    let (start, end) = keys::range(keys::settled(id));
    outbox.delete_range(start, end);
    outbox.put(keys::cut(id), &snapshot.cut);
    for it in &snapshot.values {
        let key = keys::settled_value(id, it.model, &it.key);
        outbox.put_bytes(key, it.value.clone());
    }
}

/// Deletes everything that hasn't settled, for a rewind.
pub(super) fn forget_frontier(id: AgreementId, outbox: &mut Outbox) {
    for prefix in [keys::logs(id), keys::frontier(id)] {
        let (start, end) = keys::range(prefix);
        outbox.delete_range(start, end);
    }
}
