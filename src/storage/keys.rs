//! The key layout. Structure lives in the keys, so range scans and point
//! reads are the only queries the library ever needs.
//!
//! ```text
//! [GENESIS][agreement]                                → Genesis
//! [AGREEMENT][agreement][PROTOCOL][VIEW]              → agreement View
//! [AGREEMENT][agreement][PROTOCOL][ATTESTATION]       → our latest Signed<Attestation>
//! [AGREEMENT][agreement][PROTOCOL][CLOCK]             → integrated clock, floor
//! [AGREEMENT][agreement][PROTOCOL][LOG][author][seq]  → Signed<Transaction>
//! [AGREEMENT][agreement][DATA][model][key…][stamp]    → KV Version
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
//! - **Protocol** state (what replication needs) and **data** (what models
//!   hold) are separate ranges. Hydration loads the protocol range, which
//!   stays small because compaction trims the log. The node never loads
//!   data at all, except to send a snapshot.
//! - An author's log is a sub-range, so compacting it is one range delete.
//! - A key's versions are a sub-range in timeline order, because a stamp's
//!   bytes sort the way stamps do. Writing a version never needs to read
//!   the others, and the last one in the range is the current one.

use crate::actor::Peer;
use crate::agreement::AgreementId;
use crate::clock::Stamp;
use crate::model::ModelId;

const GENESIS: u8 = 1;
const AGREEMENT: u8 = 2;

const PROTOCOL: u8 = 1;
const DATA: u8 = 2;

const VIEW: u8 = 1;
const ATTESTATION: u8 = 2;
const CLOCK: u8 = 3;
const LOG: u8 = 4;

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
    Version {
        agreement: AgreementId,
        model: ModelId,
        key: Vec<u8>,
        /// Boxed because a stamp holds its author's whole public key.
        stamp: Box<Stamp>,
    },
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

/// Every model's data.
pub(crate) fn data(id: AgreementId) -> Vec<u8> {
    [agreement(id), vec![DATA]].concat()
}

/// One model's data.
pub(crate) fn model(id: AgreementId, model: ModelId) -> Vec<u8> {
    [data(id), model.as_bytes().to_vec()].concat()
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

pub(crate) fn log(id: AgreementId, author: Peer, seq: u64) -> Vec<u8> {
    [log_of(id, author), seq.to_be_bytes().to_vec()].concat()
}

/// One author's transactions with `seq` in `1..=through`, as a key range.
pub(crate) fn log_through(id: AgreementId, author: Peer, through: u64) -> (Vec<u8>, Vec<u8>) {
    (log(id, author, 0), log(id, author, through + 1))
}

/// Every version of `key`, and of every key that starts with it.
pub(crate) fn versions(id: AgreementId, model_id: ModelId, key: &[u8]) -> Vec<u8> {
    [model(id, model_id), key.to_vec()].concat()
}

/// What the transaction stamped `stamp` wrote to `key`.
pub(crate) fn version(id: AgreementId, model_id: ModelId, key: &[u8], stamp: Stamp) -> Vec<u8> {
    let lamport = stamp.lamport.to_be_bytes();
    let author = stamp.author.as_bytes();

    [
        versions(id, model_id, key),
        lamport.to_vec(),
        author.to_vec(),
    ]
    .concat()
}

fn log_of(id: AgreementId, author: Peer) -> Vec<u8> {
    [protocol(id), vec![LOG], author.as_bytes().to_vec()].concat()
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
            (&DATA, rest) => parse_data(agreement, rest),
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
        _ => None,
    }
}

fn parse_data(agreement: AgreementId, key: &[u8]) -> Option<Record> {
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
        let model = ModelId::new(id(), [2; 32]);

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
    fn protocol_state_and_data_are_separate_ranges() {
        let model = ModelId::new(id(), [2; 32]);

        for key in [
            view(id()),
            attestation(id()),
            clock(id()),
            log(id(), phone(), 1),
        ] {
            assert!(key.starts_with(&protocol(id())));
        }

        assert!(versions(id(), model, b"").starts_with(&data(id())));
        assert!(!versions(id(), model, b"").starts_with(&protocol(id())));
        assert!(protocol(id()).starts_with(&agreement(id())));
        assert!(data(id()).starts_with(&agreement(id())));
    }

    #[test]
    fn a_keys_versions_sort_by_stamp() {
        let model = ModelId::new(id(), [2; 32]);
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
    fn log_keys_sort_by_sequence() {
        let (start, end) = log_through(id(), phone(), 256);

        assert!(start < log(id(), phone(), 1));
        assert!(log(id(), phone(), 255) < log(id(), phone(), 256));
        assert!(log(id(), phone(), 256) < end);
        assert!(end <= log(id(), phone(), 257));
    }
}
