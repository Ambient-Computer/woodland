//! Rebuilding a node from storage, as a conversation of scans.
//!
//! 1. [`start`]: scan the genesis records to find which agreements exist.
//! 2. [`scanned`] with the genesis list: scan each agreement's protocol
//!    state. Model data isn't loaded. The node only ever writes it.
//! 3. [`scanned`] with an agreement's entries: rebuild that group.
//!
//! The node never waits. Each step returns scans as actions, and the
//! answers come back as events in a later batch, like anything else.

use std::collections::BTreeMap;

use super::Outbox;
use crate::actor::Peer;
use crate::agreement::{AgreementId, Genesis};
use crate::encoding;
use crate::group::{Group, Restored};
use crate::storage::keys::{self, Record};

/// Agreements found in storage whose data hasn't been scanned yet.
#[derive(Debug, Default)]
pub(super) struct Hydration {
    found: BTreeMap<AgreementId, Genesis>,
}

pub(super) fn start(outbox: &mut Outbox) {
    outbox.scan(keys::geneses());
}

/// Handles a finished scan. Returns a group once its data is in.
pub(super) fn scanned(
    hydration: &mut Hydration,
    me: Peer,
    prefix: &[u8],
    entries: &[(Vec<u8>, Vec<u8>)],
    outbox: &mut Outbox,
) -> Option<Group> {
    if prefix == keys::geneses() {
        for (key, value) in entries {
            if let (Some(Record::Genesis(id)), Some(genesis)) =
                (keys::parse(key), encoding::decode::<Genesis>(value))
                && genesis.id() == id
            {
                outbox.scan(keys::protocol(id));
                hydration.found.insert(id, genesis);
            }
        }

        return None;
    }

    let id = *hydration
        .found
        .keys()
        .find(|&&id| keys::protocol(id) == prefix)?;
    let genesis = hydration.found.remove(&id)?;

    Some(Group::from(Restored::from_entries(&genesis, me, entries)))
}
