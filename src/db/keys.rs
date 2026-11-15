//! The key layout. Structure lives in the keys, so range scans and point
//! reads are the only queries the library ever needs.
//!
//! ```text
//! [IDENTITY]                                    → the device's secret key
//! [FOUNDING][group]                             → the group's first checkpoint
//! [GROUP][group][PROTOCOL][CHAIN]               → the checkpoint chain
//! [GROUP][group][PROTOCOL][META]                → what's held, what's committed
//! [GROUP][group][PROTOCOL][LOG][author][seq]    → an entry
//! [GROUP][group][COMMITTED][model][key…]        → the key's committed value
//! [GROUP][group][UNCOMMITTED][model][key…][stamp] → a version: a value, or none
//! [GROUP][group][DROPPED][model][stamp]         → the model was dropped, uncommitted
//! ```
//!
//! Identifiers are fixed width and counters are big-endian, so byte order
//! matches logical order. The application's key bytes are the only part
//! that varies in length, and everything around them is fixed width, which
//! keeps them unambiguous without any escaping, and keeps prefix queries
//! over them a single range.
//!
//! The payoff:
//!
//! - Everything a group owns shares one prefix, so dropping a group is
//!   one range.
//! - **Protocol** state (what replication needs) and model data are
//!   separate ranges. Opening a group reads protocol state and never
//!   model data.
//! - Model data is split by whether it can still change. The
//!   **committed** range holds one value per key. The **uncommitted**
//!   range holds what entries no checkpoint covers have written.
//! - A model that's dropped has to read as empty before the drop is
//!   committed, without touching what it holds, in case the drop doesn't
//!   happen. The **dropped** range says so, and a reader checks it.
//! - A key's uncommitted versions are a sub-range in timeline order,
//!   because a stamp's bytes sort the way stamps do. Writing a version
//!   never needs to read the others, and the last one in the range is
//!   the current one.

use crate::actor::Peer;
use crate::clock::Stamp;
use crate::db::GroupId;
use crate::model::ModelId;

const FOUNDING: u8 = 1;
const GROUP: u8 = 2;
const IDENTITY: u8 = 3;

const PROTOCOL: u8 = 1;
const COMMITTED: u8 = 2;
const UNCOMMITTED: u8 = 3;
const DROPPED: u8 = 4;

const CHAIN: u8 = 1;
const META: u8 = 2;
const LOG: u8 = 3;

/// A stored key of model data, parsed back into what it names.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Record {
    Committed {
        group: GroupId,
        model: ModelId,
        key: Vec<u8>,
    },
    Version {
        group: GroupId,
        model: ModelId,
        key: Vec<u8>,
        /// Boxed because a stamp holds its author's whole public key.
        stamp: Box<Stamp>,
    },
    Dropped {
        group: GroupId,
        model: ModelId,
    },
}

/// The device's secret key. It lives beside everything it signed, so a
/// store that's lost takes the key with it.
pub(crate) fn identity() -> Vec<u8> {
    vec![IDENTITY]
}

/// Where every group's first checkpoint lives. Scanning it lists the
/// groups.
pub(crate) fn foundings() -> Vec<u8> {
    vec![FOUNDING]
}

pub(crate) fn founding(group: GroupId) -> Vec<u8> {
    [&[FOUNDING][..], group.as_bytes()].concat()
}

/// Everything a group owns.
pub(crate) fn group(group: GroupId) -> Vec<u8> {
    [&[GROUP][..], group.as_bytes()].concat()
}

/// Everything replication needs to resume.
pub(crate) fn protocol(id: GroupId) -> Vec<u8> {
    [group(id), vec![PROTOCOL]].concat()
}

pub(crate) fn chain(id: GroupId) -> Vec<u8> {
    [protocol(id), vec![CHAIN]].concat()
}

pub(crate) fn meta(id: GroupId) -> Vec<u8> {
    [protocol(id), vec![META]].concat()
}

/// Every entry held.
pub(crate) fn logs(id: GroupId) -> Vec<u8> {
    [protocol(id), vec![LOG]].concat()
}

pub(crate) fn log(id: GroupId, author: Peer, seq: u64) -> Vec<u8> {
    let author = author.as_bytes().to_vec();

    [logs(id), author, seq.to_be_bytes().to_vec()].concat()
}

/// Every key that starts with `prefix`, as a key range. The prefix must
/// not end in `0xFF`. None of this module's do.
pub(crate) fn range(mut prefix: Vec<u8>) -> (Vec<u8>, Vec<u8>) {
    let start = prefix.clone();
    if let Some(last) = prefix.last_mut() {
        *last += 1;
    }

    (start, prefix)
}

/// The smallest key greater than `key`.
pub(crate) fn after(key: &[u8]) -> Vec<u8> {
    [key, &[0]].concat()
}

/// Every model's committed values. What a snapshot is made of.
pub(crate) fn committed(id: GroupId) -> Vec<u8> {
    [group(id), vec![COMMITTED]].concat()
}

/// The committed value of `key`, and of every key that starts with it.
pub(crate) fn committed_value(id: GroupId, model: ModelId, key: &[u8]) -> Vec<u8> {
    let model = model.as_bytes().to_vec();

    [committed(id), model, key.to_vec()].concat()
}

/// Everything written by entries that aren't committed.
pub(crate) fn uncommitted(id: GroupId) -> Vec<u8> {
    [group(id), vec![UNCOMMITTED]].concat()
}

/// Every version of `key`, and of every key that starts with it.
pub(crate) fn versions(id: GroupId, model: ModelId, key: &[u8]) -> Vec<u8> {
    [uncommitted(id), model.as_bytes().to_vec(), key.to_vec()].concat()
}

/// What the entry stamped `stamp` wrote to `key`.
pub(crate) fn version(id: GroupId, model: ModelId, key: &[u8], stamp: Stamp) -> Vec<u8> {
    let lamport = stamp.lamport.to_be_bytes();
    let author = stamp.author.as_bytes();

    [versions(id, model, key), lamport.to_vec(), author.to_vec()].concat()
}

/// Every model dropped by an entry that isn't committed.
pub(crate) fn dropped(id: GroupId) -> Vec<u8> {
    [group(id), vec![DROPPED]].concat()
}

/// Whether `model` was dropped by an entry that isn't committed: one
/// key for each that did.
pub(crate) fn drops(id: GroupId, model: ModelId) -> Vec<u8> {
    [dropped(id), model.as_bytes().to_vec()].concat()
}

/// The entry stamped `stamp` dropped `model`.
pub(crate) fn drop(id: GroupId, model: ModelId, stamp: Stamp) -> Vec<u8> {
    let lamport = stamp.lamport.to_be_bytes();
    let author = stamp.author.as_bytes();

    [drops(id, model), lamport.to_vec(), author.to_vec()].concat()
}

/// Reads a key of model data written by this module. `None` means it
/// isn't one.
pub(crate) fn parse(key: &[u8]) -> Option<Record> {
    let (&space, rest) = key.split_first()?;
    let (group, rest) = take_id(rest)?;
    let group = GroupId::from_bytes(group);
    let (&part, rest) = rest.split_first()?;
    let (model, key) = take_id(rest)?;
    let model = ModelId::from_bytes(model);
    if space != GROUP {
        return None;
    }

    match part {
        COMMITTED => Some(Record::Committed {
            group,
            model,
            key: key.to_vec(),
        }),

        UNCOMMITTED => {
            let (key, author) = key.split_last_chunk::<32>()?;
            let (key, lamport) = key.split_last_chunk::<8>()?;
            let stamp = Stamp {
                lamport: u64::from_be_bytes(*lamport),
                author: Peer::from_bytes(author)?,
            };

            Some(Record::Version {
                group,
                model,
                key: key.to_vec(),
                stamp: Box::new(stamp),
            })
        }

        DROPPED => Some(Record::Dropped { group, model }),
        _ => None,
    }
}

fn take_id(bytes: &[u8]) -> Option<([u8; 32], &[u8])> {
    let (id, rest) = bytes.split_first_chunk::<32>()?;

    Some((*id, rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::peers::{laptop, phone};

    fn id() -> GroupId {
        GroupId::from_bytes([1; 32])
    }

    fn stamp(lamport: u64, author: Peer) -> Stamp {
        Stamp { lamport, author }
    }

    #[test]
    fn round_trips_model_data() {
        let model = ModelId::kv(id(), [2; 32]);

        assert_eq!(
            parse(&committed_value(id(), model, b"color")),
            Some(Record::Committed {
                group: id(),
                model,
                key: b"color".to_vec(),
            }),
        );
        assert_eq!(
            parse(&version(id(), model, b"color", stamp(3, phone()))),
            Some(Record::Version {
                group: id(),
                model,
                key: b"color".to_vec(),
                stamp: Box::new(stamp(3, phone())),
            }),
        );
        assert_eq!(
            parse(&drop(id(), model, stamp(3, phone()))),
            Some(Record::Dropped { group: id(), model }),
        );
        assert_eq!(parse(&log(id(), phone(), 9)), None);
        assert_eq!(parse(&founding(id())), None);
    }

    #[test]
    fn protocol_state_and_each_kind_of_model_data_are_separate_ranges() {
        let model = ModelId::kv(id(), [2; 32]);

        for key in [chain(id()), meta(id()), log(id(), phone(), 1)] {
            assert!(key.starts_with(&protocol(id())));
        }

        assert!(committed_value(id(), model, b"").starts_with(&committed(id())));
        assert!(versions(id(), model, b"").starts_with(&uncommitted(id())));
        assert!(drop(id(), model, stamp(1, phone())).starts_with(&drops(id(), model)));
        assert!(drops(id(), model).starts_with(&dropped(id())));
        let ranges = [
            protocol(id()),
            committed(id()),
            uncommitted(id()),
            dropped(id()),
        ];
        for range in &ranges {
            assert!(range.starts_with(&group(id())));
            let overlapping = ranges.iter().filter(|other| other.starts_with(range));
            assert_eq!(overlapping.count(), 1);
        }
    }

    #[test]
    fn a_keys_versions_sort_by_stamp() {
        let model = ModelId::kv(id(), [2; 32]);
        let mut stamps = [
            stamp(1, phone()),
            stamp(1, laptop()),
            stamp(2, phone()),
            stamp(256, laptop()),
        ];
        stamps.sort();

        let stored = stamps.map(|stamp| version(id(), model, b"color", stamp));
        assert!(stored.is_sorted());
        for key in &stored {
            assert!(key.starts_with(&versions(id(), model, b"color")));
        }
    }

    #[test]
    fn a_prefixes_range_holds_exactly_its_keys() {
        let model = ModelId::kv(id(), [2; 32]);
        let (start, end) = range(logs(id()));

        assert!(start <= log(id(), phone(), 0));
        assert!(log(id(), laptop(), u64::MAX) < end);
        assert!(meta(id()) < start);
        assert!(end < committed_value(id(), model, b""));
        assert!(end < versions(id(), model, b""));
    }

    #[test]
    fn log_keys_sort_by_sequence() {
        assert!(log(id(), phone(), 255) < log(id(), phone(), 256));
        assert!(log(id(), phone(), 256) < log(id(), phone(), 257));
    }
}
