//! Rebuilding a node from storage, as a conversation of scans.
//!
//! 1. [`start`]: scan the genesis records to find which agreements exist.
//! 2. [`scanned`] with the genesis list: start scanning each agreement's
//!    protocol state. Model data isn't loaded. The node only ever writes
//!    it.
//! 3. [`scanned`] with a page of an agreement's entries: take it in, and
//!    scan the next page, or rebuild the group if that was the last.
//!
//! The protocol range holds the log, which can be long. It's read a page
//! at a time, and only an outline of each transaction is kept, so
//! starting up never needs the whole log in memory at once.
//!
//! The node never waits. Each step returns scans as actions, and the
//! answers come back as events in a later batch, like anything else.

use std::collections::BTreeMap;

use super::Outbox;
use crate::actor::Peer;
use crate::agreement::{AgreementId, Genesis};
use crate::encoding;
use crate::group::{Group, Restored};
use crate::storage::PAGE;
use crate::storage::keys::{self, Record};

/// Agreements found in storage, and what's been read of each so far.
#[derive(Debug, Default)]
pub(super) struct Hydration {
    found: BTreeMap<AgreementId, Restored>,
}

pub(super) fn start(outbox: &mut Outbox) {
    outbox.scan(keys::geneses());
}

/// Handles a finished scan of `start..end`. Returns a group once all of
/// its data is in.
pub(super) fn scanned(
    hydration: &mut Hydration,
    me: Peer,
    (start, end): (&[u8], &[u8]),
    entries: &[(Vec<u8>, Vec<u8>)],
    outbox: &mut Outbox,
) -> Option<Group> {
    let (first, last) = keys::range(keys::geneses());
    if (start, end) == (&first[..], &last[..]) {
        for (key, value) in entries {
            if let (Some(Record::Genesis(id)), Some(genesis)) =
                (keys::parse(key), encoding::decode::<Genesis>(value))
                && genesis.id() == id
            {
                let (start, end) = keys::range(keys::protocol(id));
                outbox.scan_page(start, end, PAGE);
                hydration.found.insert(id, Restored::fresh(&genesis, me));
            }
        }

        return None;
    }

    // Each page starts wherever the last one stopped, and they all end
    // where the agreement's protocol range does.
    let within = |id: &AgreementId| {
        let (first, last) = keys::range(keys::protocol(*id));

        first[..] <= *start && last == end
    };

    let id = hydration.found.keys().copied().find(within)?;
    hydration.found.get_mut(&id)?.take(entries);
    if let Some((last, _)) = entries.last()
        && entries.len() >= PAGE
    {
        outbox.scan_page(keys::after(last), end.to_vec(), PAGE);
        return None;
    }

    hydration.found.remove(&id).map(Group::from)
}
