//! The key layout. Structure lives in the keys, so range scans and point
//! reads are the only queries the library ever needs.
//!
//! ```text
//! [GENESIS][agreement]                                → Genesis
//! [AGREEMENT][agreement][PROTOCOL][VIEW]              → agreement View
//! [AGREEMENT][agreement][PROTOCOL][ATTESTATION]       → our latest Signed<Attestation>
//! [AGREEMENT][agreement][PROTOCOL][CLOCK]             → integrated clock, settled cut
//! [AGREEMENT][agreement][PROTOCOL][LOG][author][seq]  → Signed<Transaction>
//! [AGREEMENT][agreement][PROTOCOL][HELD][index]       → a write held until admission
//! [AGREEMENT][agreement][SETTLED][CUT]                → which transactions are settled
//! [AGREEMENT][agreement][SETTLED][DATA][model][key…]  → the key's settled value
//! [AGREEMENT][agreement][FRONTIER][model][key…][stamp] → a version: a value, or none
//! [AGREEMENT][agreement][DROPPED][model][stamp]        → the model was dropped, unsettled
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
//! - Everything an agreement owns shares one prefix, so dropping an
//!   agreement is one range.
//! - **Protocol** state (what replication needs) and model data are
//!   separate ranges. Hydration loads the protocol range, which stays small
//!   because settling trims the log. The node never loads model data at
//!   all, except to send a snapshot.
//! - Model data is split by whether it can still change. The **settled**
//!   range holds one value per key and the cut it reflects, so one scan of
//!   it is a consistent snapshot, whenever the scan happens to run. The
//!   **frontier** holds what transactions above the cut wrote.
//! - A model that's dropped has to read as empty before the drop
//!   settles, without touching what it holds, in case the drop is pruned.
//!   The **dropped** range says so, and a reader checks it.
//! - A key's frontier versions are a sub-range in timeline order, because a
//!   stamp's bytes sort the way stamps do. Writing a version never needs to
//!   read the others, and the last one in the range is the current one.

use crate::actor::Peer;
use crate::agreement::AgreementId;
use crate::clock::Stamp;
use crate::model::ModelId;

const GENESIS: u8 = 1;
const AGREEMENT: u8 = 2;
const IDENTITY: u8 = 3;

const PROTOCOL: u8 = 1;
const SETTLED: u8 = 2;
const FRONTIER: u8 = 3;
const DROPPED: u8 = 4;

const CUT: u8 = 1;
const DATA: u8 = 2;

const VIEW: u8 = 1;
const ATTESTATION: u8 = 2;
const CLOCK: u8 = 3;
const LOG: u8 = 4;
const HELD: u8 = 5;

/// A stored key, parsed back into what it names.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Record {
    Genesis(AgreementId),
    View(AgreementId),
    Attestation(AgreementId),
    Clock(AgreementId),
    Log {
        agreement: AgreementId,
        author: [u8; 32],
        seq: u64,
    },
    Held {
        agreement: AgreementId,
        index: u64,
    },
    Cut(AgreementId),
    Settled {
        agreement: AgreementId,
        model: ModelId,
        key: Vec<u8>,
    },
    Version {
        agreement: AgreementId,
        model: ModelId,
        key: Vec<u8>,
        /// Boxed because a stamp holds its author's whole public key.
        stamp: Box<Stamp>,
    },
    Dropped {
        agreement: AgreementId,
        model: ModelId,
    },
}

/// The device's secret key. It lives beside everything it signed, so a
/// store that's lost takes the key with it.
pub(crate) fn identity() -> Vec<u8> {
    vec![IDENTITY]
}

/// Where every agreement's genesis lives. Scanning it lists the agreements.
pub(crate) fn geneses() -> Vec<u8> {
    vec![GENESIS]
}

pub(crate) fn genesis(agreement: AgreementId) -> Vec<u8> {
    [&[GENESIS][..], agreement.as_bytes()].concat()
}

/// Everything an agreement owns.
pub(crate) fn agreement(agreement: AgreementId) -> Vec<u8> {
    [&[AGREEMENT][..], agreement.as_bytes()].concat()
}

/// Everything replication needs to resume: what hydration loads.
pub(crate) fn protocol(id: AgreementId) -> Vec<u8> {
    [agreement(id), vec![PROTOCOL]].concat()
}

/// Every transaction in the log.
pub(crate) fn logs(id: AgreementId) -> Vec<u8> {
    [protocol(id), vec![LOG]].concat()
}

/// Every key that starts with `prefix`, as a key range. The prefix must
/// not end in `0xFF`. None of this module's do.
pub(crate) fn range(mut prefix: Vec<u8>) -> (Vec<u8>, Vec<u8>) {
    let start = prefix.clone();
    if let Some(last) = prefix.last_mut() {
        *last = last.saturating_add(1);
    }

    (start, prefix)
}

/// The first key after `key`, to carry on a scan from where it stopped.
pub(crate) fn after(key: &[u8]) -> Vec<u8> {
    [key, &[0]].concat()
}

/// Everything settled: the cut, and every model's settled values. What a
/// snapshot is made of.
pub(crate) fn settled(id: AgreementId) -> Vec<u8> {
    [agreement(id), vec![SETTLED]].concat()
}

/// Which transactions the settled values reflect.
pub(crate) fn cut(id: AgreementId) -> Vec<u8> {
    [settled(id), vec![CUT]].concat()
}

/// The settled value of `key`, and of every key that starts with it.
pub(crate) fn settled_value(id: AgreementId, model: ModelId, key: &[u8]) -> Vec<u8> {
    let model = model.as_bytes().to_vec();

    [settled(id), vec![DATA], model, key.to_vec()].concat()
}

/// Everything written by transactions that haven't settled.
pub(crate) fn frontier(id: AgreementId) -> Vec<u8> {
    [agreement(id), vec![FRONTIER]].concat()
}

/// Every model dropped by a transaction that hasn't settled.
pub(crate) fn dropped(id: AgreementId) -> Vec<u8> {
    [agreement(id), vec![DROPPED]].concat()
}

/// Whether `model` was dropped by a transaction that hasn't settled: one
/// entry for each that did.
pub(crate) fn drops(id: AgreementId, model: ModelId) -> Vec<u8> {
    [dropped(id), model.as_bytes().to_vec()].concat()
}

/// The transaction stamped `stamp` dropped `model`.
pub(crate) fn drop(id: AgreementId, model: ModelId, stamp: Stamp) -> Vec<u8> {
    let lamport = stamp.lamport.to_be_bytes();
    let author = stamp.author.as_bytes();

    [drops(id, model), lamport.to_vec(), author.to_vec()].concat()
}

pub(crate) fn view(id: AgreementId) -> Vec<u8> {
    [protocol(id), vec![VIEW]].concat()
}

pub(crate) fn attestation(id: AgreementId) -> Vec<u8> {
    [protocol(id), vec![ATTESTATION]].concat()
}

pub(crate) fn clock(id: AgreementId) -> Vec<u8> {
    [protocol(id), vec![CLOCK]].concat()
}

pub(crate) fn held(id: AgreementId, index: u64) -> Vec<u8> {
    [protocol(id), vec![HELD], index.to_be_bytes().to_vec()].concat()
}

/// Held writes with an index below `next`, as a key range.
pub(crate) fn held_through(id: AgreementId, next: u64) -> (Vec<u8>, Vec<u8>) {
    (held(id, 0), held(id, next))
}

pub(crate) fn log(id: AgreementId, author: Peer, seq: u64) -> Vec<u8> {
    [log_of(id, author), seq.to_be_bytes().to_vec()].concat()
}

/// Every version of `key`, and of every key that starts with it.
pub(crate) fn versions(id: AgreementId, model: ModelId, key: &[u8]) -> Vec<u8> {
    [frontier(id), model.as_bytes().to_vec(), key.to_vec()].concat()
}

/// What the transaction stamped `stamp` wrote to `key`.
pub(crate) fn version(id: AgreementId, model: ModelId, key: &[u8], stamp: Stamp) -> Vec<u8> {
    let lamport = stamp.lamport.to_be_bytes();
    let author = stamp.author.as_bytes();

    [versions(id, model, key), lamport.to_vec(), author.to_vec()].concat()
}

fn log_of(id: AgreementId, author: Peer) -> Vec<u8> {
    [logs(id), author.as_bytes().to_vec()].concat()
}

/// Reads a key written by this module. `None` means it isn't one of ours.
pub(crate) fn parse(key: &[u8]) -> Option<Record> {
    let (&space, rest) = key.split_first()?;
    let (agreement, rest) = take_id(rest)?;
    let agreement = AgreementId::from_bytes(agreement);

    match space {
        GENESIS if rest.is_empty() => Some(Record::Genesis(agreement)),
        AGREEMENT => match rest.split_first()? {
            (&PROTOCOL, rest) => parse_protocol(agreement, rest),
            (&SETTLED, rest) => parse_settled(agreement, rest),
            (&FRONTIER, rest) => parse_version(agreement, rest),
            (&DROPPED, rest) => {
                let (model, _) = take_id(rest)?;
                let model = ModelId::from_bytes(model);

                Some(Record::Dropped { agreement, model })
            }
            _ => None,
        },
        _ => None,
    }
}

fn parse_protocol(agreement: AgreementId, key: &[u8]) -> Option<Record> {
    let (&record, rest) = key.split_first()?;
    match record {
        VIEW if rest.is_empty() => Some(Record::View(agreement)),
        ATTESTATION if rest.is_empty() => Some(Record::Attestation(agreement)),
        CLOCK if rest.is_empty() => Some(Record::Clock(agreement)),
        LOG => {
            let (author, rest) = take_id(rest)?;
            let seq = u64::from_be_bytes(rest.try_into().ok()?);

            Some(Record::Log {
                agreement,
                author,
                seq,
            })
        }
        HELD => {
            let index = u64::from_be_bytes(rest.try_into().ok()?);

            Some(Record::Held { agreement, index })
        }
        _ => None,
    }
}

fn parse_settled(agreement: AgreementId, key: &[u8]) -> Option<Record> {
    let (&record, rest) = key.split_first()?;
    match record {
        CUT if rest.is_empty() => Some(Record::Cut(agreement)),
        DATA => {
            let (model, key) = take_id(rest)?;

            Some(Record::Settled {
                agreement,
                model: ModelId::from_bytes(model),
                key: key.to_vec(),
            })
        }
        _ => None,
    }
}

fn parse_version(agreement: AgreementId, key: &[u8]) -> Option<Record> {
    let (model, key) = take_id(key)?;
    let (key, author) = key.split_last_chunk::<32>()?;
    let (key, lamport) = key.split_last_chunk::<8>()?;

    Some(Record::Version {
        agreement,
        model: ModelId::from_bytes(model),
        key: key.to_vec(),
        stamp: Box::new(Stamp {
            lamport: u64::from_be_bytes(*lamport),
            author: Peer::from_bytes(author)?,
        }),
    })
}

fn take_id(bytes: &[u8]) -> Option<([u8; 32], &[u8])> {
    let (id, rest) = bytes.split_first_chunk::<32>()?;

    Some((*id, rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agreement::Genesis;
    use crate::testing::peers::{laptop, phone};

    fn id() -> AgreementId {
        Genesis::new([1; 32], [phone().id()]).id()
    }

    fn stamp(lamport: u64, author: Peer) -> Stamp {
        Stamp { lamport, author }
    }

    #[test]
    fn round_trips_every_record() {
        let model = ModelId::kv(id(), [2; 32]);

        assert_eq!(parse(&genesis(id())), Some(Record::Genesis(id())));
        assert_eq!(parse(&view(id())), Some(Record::View(id())));
        assert_eq!(parse(&clock(id())), Some(Record::Clock(id())));
        assert_eq!(parse(&attestation(id())), Some(Record::Attestation(id())));
        assert_eq!(
            parse(&log(id(), phone(), 9)),
            Some(Record::Log {
                agreement: id(),
                author: *phone().as_bytes(),
                seq: 9
            }),
        );
        assert_eq!(
            parse(&held(id(), 4)),
            Some(Record::Held {
                agreement: id(),
                index: 4
            }),
        );
        assert_eq!(parse(&cut(id())), Some(Record::Cut(id())));
        assert_eq!(
            parse(&settled_value(id(), model, b"color")),
            Some(Record::Settled {
                agreement: id(),
                model,
                key: b"color".to_vec(),
            }),
        );
        assert_eq!(
            parse(&version(id(), model, b"color", stamp(3, phone()))),
            Some(Record::Version {
                agreement: id(),
                model,
                key: b"color".to_vec(),
                stamp: Box::new(stamp(3, phone())),
            }),
        );
    }

    #[test]
    fn protocol_state_settled_data_and_the_frontier_are_separate_ranges() {
        let model = ModelId::kv(id(), [2; 32]);

        for key in [
            view(id()),
            attestation(id()),
            clock(id()),
            log(id(), phone(), 1),
            held(id(), 1),
        ] {
            assert!(key.starts_with(&protocol(id())));
        }

        for key in [cut(id()), settled_value(id(), model, b"")] {
            assert!(key.starts_with(&settled(id())));
        }

        assert!(versions(id(), model, b"").starts_with(&frontier(id())));
        let ranges = [protocol(id()), settled(id()), frontier(id()), dropped(id())];
        assert!(drop(id(), model, stamp(1, phone())).starts_with(&drops(id(), model)));
        assert!(drops(id(), model).starts_with(&dropped(id())));
        for range in &ranges {
            assert!(range.starts_with(&agreement(id())));
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
        assert!(clock(id()) < start);
        assert!(end <= held(id(), 0));
        assert!(end < cut(id()));
        assert!(end < versions(id(), model, b""));
    }

    #[test]
    fn log_keys_sort_by_sequence() {
        assert!(log(id(), phone(), 255) < log(id(), phone(), 256));
        assert!(log(id(), phone(), 256) < log(id(), phone(), 257));
    }
}
