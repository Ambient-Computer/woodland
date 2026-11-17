//! The key layout. Structure lives in the keys, so range scans and point
//! reads are the only queries the library ever needs.
//!
//! ```text
//! [IDENTITY]                                    → the device's secret key
//! [FOUNDING][group]                             → the group's first checkpoint
//! [GROUP][group][PROTOCOL][CHAIN]               → where the checkpoint chain began, and what's undecided
//! [GROUP][group][PROTOCOL][LINK][height]        → a decided checkpoint, and who voted for it
//! [GROUP][group][PROTOCOL][META]                → what's held, what's committed
//! [GROUP][group][PROTOCOL][LOG][clock][author][seq] → an entry
//! [GROUP][group][PROTOCOL][PROOF][author]     → proof that the author forked
//! [GROUP][group][PROTOCOL][STATE][clock]        → the sum the clock's state root is the hash of
//! [GROUP][group][COMMITTED][model][key…]        → the key's committed value
//! [GROUP][group][UNCOMMITTED][model][key…][end][stamp] → a version: a value, or none
//! [GROUP][group][STAGED][model][key…]           → a value from a snapshot not yet checked
//! [GROUP][group][SEALED][cursor]                → a piece of a snapshot this device can't read
//! [GROUP][group][TRIAL][model][key…]            → what a transaction being tried would leave
//! [GROUP][group][AHEAD][the rest of a key…]     → what a checkpoint not yet decided would leave
//! [GROUP][group][BEFORE][the rest of a key…]    → what the key held before a checkpoint that's partly in place
//! ```
//!
//! A model's ID starts with its clock's, so everything a clock owns is
//! one range within each of the last three.
//!
//! Identifiers are fixed width and counters are big-endian, so byte order
//! matches logical order. The application's key bytes are the only part
//! that varies in length. Where nothing follows them they're stored as
//! they are, and a prefix query over them is a single range.
//!
//! A version's key is followed by a stamp, so there it's written to
//! say where it ends (see [`escape`]). Otherwise a short key's versions
//! would sort in among those of the longer keys it starts, and the
//! versions of a range of keys wouldn't be a range.
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
//! - A key's uncommitted versions are a sub-range in timeline order,
//!   because a stamp's bytes sort the way stamps do. Writing a version
//!   never needs to read the others, and the last one in the range is
//!   the current one.

use crate::actor::Peer;
use crate::clock::Stamp;
use crate::db::{ClockId, GroupId};
use crate::model::ModelId;
use crate::model::table::{escape, successor, unescape};

const FOUNDING: u8 = 1;
const GROUP: u8 = 2;
const IDENTITY: u8 = 3;
const SEALING: u8 = 4;

const PROTOCOL: u8 = 1;
const COMMITTED: u8 = 2;
const UNCOMMITTED: u8 = 3;
const STAGED: u8 = 5;
const SEALED: u8 = 6;
const TRIAL: u8 = 7;
const AHEAD: u8 = 8;
const INDEX: u8 = 9;
const INDEXED: u8 = 10;
const INDEXES: u8 = 11;
const BEFORE: u8 = 12;

const CHAIN: u8 = 1;
const META: u8 = 2;
const LOG: u8 = 3;
const STATE: u8 = 4;
const PROOF: u8 = 5;
const LINK: u8 = 6;
const VEIL: u8 = 7;
const OUTCOME: u8 = 8;

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
    /// A row's place in an index this device keeps.
    Index {
        group: GroupId,
        model: ModelId,
        key: Vec<u8>,
        /// Whether it's the index of what's committed.
        committed: bool,
    },
}

impl Record {
    /// The row of a table this says exists, if that's what it says.
    pub(crate) fn row(self) -> Option<Vec<u8>> {
        let (Self::Committed { key, .. } | Self::Version { key, .. }) = self else {
            return None;
        };

        match crate::model::table::Place::parse(&key)? {
            crate::model::table::Place::Row(row) => Some(row),
            _ => None,
        }
    }
}

/// The device's secret key. It lives beside everything it signed, so a
/// store that's lost takes the key with it.
pub(crate) fn identity() -> Vec<u8> {
    vec![IDENTITY]
}

/// What the device's sealing keys since its first were made from.
pub(crate) fn sealing() -> Vec<u8> {
    vec![SEALING]
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

/// What last became of a transaction this device wrote.
pub(crate) fn outcome(id: GroupId, clock: ClockId, author: Peer, seq: u64) -> Vec<u8> {
    let clock = clock.as_bytes().to_vec();
    let author = author.as_bytes().to_vec();
    let seq = seq.to_be_bytes().to_vec();

    [protocol(id), vec![OUTCOME], clock, author, seq].concat()
}

/// Whether reads are being kept from a checkpoint that's partly in
/// place, and how.
pub(crate) fn veil(id: GroupId) -> Vec<u8> {
    [protocol(id), vec![VEIL]].concat()
}

/// What keys held before a checkpoint that's partly in place. What
/// follows is the key, less the group it starts with.
pub(crate) fn before(id: GroupId) -> Vec<u8> {
    [group(id), vec![BEFORE]].concat()
}

/// The parts of a group that reads are answered from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layer {
    /// What's committed, and the index of it.
    Committed,
    /// What entries no checkpoint covers have written.
    Versions,
    /// The index of everything a device shows.
    Shown,
}

/// Which part of a group a key is in, if it's one that reads are
/// answered from. `rest` is the key, less the group it starts with.
pub(crate) fn layer(rest: &[u8]) -> Option<Layer> {
    match rest {
        [COMMITTED, ..] | [INDEX, 1, ..] => Some(Layer::Committed),
        [UNCOMMITTED, ..] => Some(Layer::Versions),
        [INDEX, 0, ..] => Some(Layer::Shown),
        _ => None,
    }
}

/// The running sum of everything committed in a clock.
pub(crate) fn state(id: GroupId, clock: ClockId) -> Vec<u8> {
    [protocol(id), vec![STATE], clock.as_bytes().to_vec()].concat()
}

/// Every decided checkpoint held, in order.
pub(crate) fn links(id: GroupId) -> Vec<u8> {
    [protocol(id), vec![LINK]].concat()
}

/// The checkpoint decided at `height`.
pub(crate) fn link(id: GroupId, height: u64) -> Vec<u8> {
    [links(id), height.to_be_bytes().to_vec()].concat()
}

/// Every fork proof held.
pub(crate) fn proofs(id: GroupId) -> Vec<u8> {
    [protocol(id), vec![PROOF]].concat()
}

/// The proof that `author` forked. One is enough.
pub(crate) fn proof(id: GroupId, author: Peer) -> Vec<u8> {
    [proofs(id), author.as_bytes().to_vec()].concat()
}

/// Every entry held.
pub(crate) fn logs(id: GroupId) -> Vec<u8> {
    [protocol(id), vec![LOG]].concat()
}

pub(crate) fn log(id: GroupId, clock: ClockId, author: Peer, seq: u64) -> Vec<u8> {
    let clock = clock.as_bytes().to_vec();
    let author = author.as_bytes().to_vec();

    [logs(id), clock, author, seq.to_be_bytes().to_vec()].concat()
}

/// Every key that starts with `prefix`, as a key range. A prefix may
/// end in `0xFF`, as one that ends in an ID will one time in 256: the
/// range then ends at the first key past the bytes before it.
///
/// # Panics
///
/// If the prefix is nothing but `0xFF`, which has nothing past it.
/// Every key starts with a tag, and no tag is `0xFF`.
pub(crate) fn range(prefix: Vec<u8>) -> (Vec<u8>, Vec<u8>) {
    let end = successor(&prefix).expect("a key starts with a tag, and no tag is 0xFF");

    (prefix, end)
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
    let mut starts = escape(key);
    starts.truncate(starts.len() - ENDED.len());

    [uncommitted(id), model.as_bytes().to_vec(), starts].concat()
}

/// What [`escape`] puts after a key to say it's over.
const ENDED: [u8; 2] = [0, 0];

/// What the entry stamped `stamp` wrote to `key`.
pub(crate) fn version(id: GroupId, model: ModelId, key: &[u8], stamp: Stamp) -> Vec<u8> {
    let lamport = stamp.lamport.to_be_bytes();
    let author = stamp.author.as_bytes();

    [
        versions(id, model, key),
        ENDED.to_vec(),
        lamport.to_vec(),
        author.to_vec(),
    ]
    .concat()
}

/// Every value of a snapshot that's still arriving.
pub(crate) fn staged(id: GroupId) -> Vec<u8> {
    [group(id), vec![STAGED]].concat()
}

/// What a transaction being tried out would write.
pub(crate) fn trial(id: GroupId) -> Vec<u8> {
    [group(id), vec![TRIAL]].concat()
}

/// What a transaction being tried out would leave under `key`.
pub(crate) fn trial_value(id: GroupId, model: ModelId, key: &[u8]) -> Vec<u8> {
    [trial(id), model.as_bytes().to_vec(), key.to_vec()].concat()
}

/// Everything working out a checkpoint that isn't decided would write,
/// under the rest of the key it would write it to.
pub(crate) fn ahead(id: GroupId) -> Vec<u8> {
    [group(id), vec![AHEAD]].concat()
}

/// A snapshot of a sealed group, as a device that can't read it
/// keeps it: in the pieces it came in.
pub(crate) fn sealed(id: GroupId) -> Vec<u8> {
    [group(id), vec![SEALED]].concat()
}

/// A value of a snapshot that's still arriving. `key` is a model and
/// then the model's own key.
pub(crate) fn staged_value(id: GroupId, key: &[u8]) -> Vec<u8> {
    [&staged(id)[..], key].concat()
}

/// Reads a key of model data written by this module. `None` means it
/// isn't one.
pub(crate) fn parse(key: &[u8]) -> Option<Record> {
    let (&space, rest) = key.split_first()?;
    let (group, rest) = take_id(rest)?;
    let group = GroupId::from_bytes(group);
    let (&part, rest) = rest.split_first()?;
    let (level, rest) = match (part, rest.split_first()) {
        (INDEX, Some((&level, rest))) => (level, rest),
        _ => (0, rest),
    };
    let (model, key) = rest.split_first_chunk::<64>()?;
    let model = ModelId::from_bytes(*model);
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
            let (key, stamp) = unescape(key)?;
            let (lamport, author) = stamp.split_first_chunk::<8>()?;
            let author: &[u8; 32] = author.try_into().ok()?;
            let stamp = Stamp {
                lamport: u64::from_be_bytes(*lamport),
                author: Peer::from_bytes(author)?,
            };

            Some(Record::Version {
                group,
                model,
                key,
                stamp: Box::new(stamp),
            })
        }

        INDEX => Some(Record::Index {
            group,
            model,
            key: key.to_vec(),
            committed: level == 1,
        }),

        _ => None,
    }
}

/// The indexes this device keeps of the group's tables, and how far
/// along each is.
pub(crate) fn indexes(id: GroupId) -> Vec<u8> {
    [group(id), vec![INDEXES]].concat()
}

/// A row's place in an index of `model`, at one level of read. `key`
/// is the column, the value the row holds in it, and the row.
pub(crate) fn index(id: GroupId, committed: bool, model: ModelId, key: &[u8]) -> Vec<u8> {
    let level = vec![INDEX, u8::from(committed)];

    [group(id), level, model.as_bytes().to_vec(), key.to_vec()].concat()
}

/// Where it says what place a row has in an index, so the place can
/// be found again when the row changes.
pub(crate) fn indexed(
    id: GroupId,
    committed: bool,
    model: ModelId,
    column: &[u8],
    row: &[u8],
) -> Vec<u8> {
    let level = vec![INDEXED, u8::from(committed)];
    let model = model.as_bytes().to_vec();

    [group(id), level, model, column.to_vec(), row.to_vec()].concat()
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
        assert_eq!(parse(&log(id(), id().into(), phone(), 9)), None);
        assert_eq!(parse(&founding(id())), None);
    }

    #[test]
    fn protocol_state_and_each_kind_of_model_data_are_separate_ranges() {
        let model = ModelId::kv(id(), [2; 32]);

        for key in [chain(id()), meta(id()), log(id(), id().into(), phone(), 1)] {
            assert!(key.starts_with(&protocol(id())));
        }

        assert!(committed_value(id(), model, b"").starts_with(&committed(id())));
        assert!(versions(id(), model, b"").starts_with(&uncommitted(id())));
        let ranges = [protocol(id()), committed(id()), uncommitted(id())];
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

    /// A key that starts another has its versions before the other's,
    /// whatever their stamps, so the versions of a range of keys are a
    /// range.
    #[test]
    fn versions_sort_by_key_before_stamp() {
        let model = ModelId::kv(id(), [2; 32]);
        let late = stamp(u64::MAX, laptop());
        let early = stamp(0, phone());
        let keys: [&[u8]; 5] = [b"", b"a", b"a\0", b"a\0\0", b"ab"];
        let stored: Vec<_> = keys
            .iter()
            .flat_map(|key| [early, late].map(|stamp| version(id(), model, key, stamp)))
            .collect();

        assert!(stored.is_sorted());
        for (key, stored) in keys.iter().zip(stored.chunks(2)) {
            assert!(versions(id(), model, key) <= stored[0]);
            let parsed = parse(&stored[1]);
            assert!(matches!(parsed, Some(Record::Version { key: found, .. }) if found == *key));
        }
    }

    #[test]
    fn a_prefixes_range_holds_exactly_its_keys() {
        let model = ModelId::kv(id(), [2; 32]);
        let (start, end) = range(logs(id()));

        assert!(start <= log(id(), id().into(), phone(), 0));
        assert!(log(id(), id().into(), laptop(), u64::MAX) < end);
        assert!(meta(id()) < start);
        assert!(end < committed_value(id(), model, b""));
        assert!(end < versions(id(), model, b""));
    }

    /// An ID is a hash, so a prefix that ends in one ends in `0xFF`
    /// one time in 256.
    #[test]
    fn a_prefix_that_ends_in_the_last_byte_has_a_range() {
        let group = GroupId::from_bytes([0xFF; 32]);
        let model = ModelId::kv(group, [2; 32]);
        let (start, end) = range(self::group(group));

        assert!(start < end);
        assert!(start <= meta(group));
        assert!(committed_value(group, model, b"\xFF\xFF") < end);
        assert!(versions(group, model, b"\xFF") < end);
        assert!(end < foundings() || foundings() < start);

        let (start, end) = range(b"a\xFF\xFF".to_vec());

        assert_eq!((start, end), (b"a\xFF\xFF".to_vec(), b"b".to_vec()));
    }

    #[test]
    fn log_keys_sort_by_sequence() {
        assert!(log(id(), id().into(), phone(), 255) < log(id(), id().into(), phone(), 256));
        assert!(log(id(), id().into(), phone(), 256) < log(id(), id().into(), phone(), 257));
    }
}
