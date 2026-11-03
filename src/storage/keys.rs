//! The key layout. Structure lives in the keys, so a range scan is the only
//! query the library ever needs.
//!
//! ```text
//! [GENESIS][agreement]                         → Genesis
//! [AGREEMENT][agreement][VIEW]                 → agreement View
//! [AGREEMENT][agreement][ATTESTATION]          → our latest Signed<Attestation>
//! [AGREEMENT][agreement][CLOCK]                → integrated clock, floor
//! [AGREEMENT][agreement][LOG][author][seq]     → Signed<Transaction>
//! [AGREEMENT][agreement][ENTRY][model][key…]   → KV Entry
//! ```
//!
//! Identifiers are fixed width and sequence numbers are big-endian, so byte
//! order matches logical order. The application's key bytes come last,
//! which keeps them unambiguous without any escaping.
//!
//! The payoff: everything an agreement owns shares one prefix, so loading or
//! dropping an agreement is one range. An author's log is a sub-range, so
//! compacting it is one range delete.

use crate::actor::Peer;
use crate::agreement::AgreementId;
use crate::model::ModelId;

const GENESIS: u8 = 1;
const AGREEMENT: u8 = 2;

const VIEW: u8 = 1;
const CLOCK: u8 = 2;
const LOG: u8 = 3;
const ENTRY: u8 = 4;
const ATTESTATION: u8 = 5;

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
    Entry {
        agreement: AgreementId,
        model: ModelId,
        key: Vec<u8>,
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

pub(crate) fn view(id: AgreementId) -> Vec<u8> {
    [agreement(id), vec![VIEW]].concat()
}

pub(crate) fn attestation(id: AgreementId) -> Vec<u8> {
    [agreement(id), vec![ATTESTATION]].concat()
}

pub(crate) fn clock(id: AgreementId) -> Vec<u8> {
    [agreement(id), vec![CLOCK]].concat()
}

pub(crate) fn log(id: AgreementId, author: Peer, seq: u64) -> Vec<u8> {
    [log_of(id, author), seq.to_be_bytes().to_vec()].concat()
}

/// One author's transactions with `seq` in `1..=through`, as a key range.
pub(crate) fn log_through(id: AgreementId, author: Peer, through: u64) -> (Vec<u8>, Vec<u8>) {
    (log(id, author, 0), log(id, author, through + 1))
}

pub(crate) fn entry(id: AgreementId, model: ModelId, key: &[u8]) -> Vec<u8> {
    [
        agreement(id),
        vec![ENTRY],
        model.as_bytes().to_vec(),
        key.to_vec(),
    ]
    .concat()
}

fn log_of(id: AgreementId, author: Peer) -> Vec<u8> {
    [agreement(id), vec![LOG], author.as_bytes().to_vec()].concat()
}

/// Reads a key written by this module. `None` means it isn't one of ours.
pub(crate) fn parse(key: &[u8]) -> Option<Record> {
    let (&space, rest) = key.split_first()?;
    let (agreement, rest) = take_id(rest)?;
    let agreement = AgreementId::from_bytes(agreement);

    match space {
        GENESIS if rest.is_empty() => Some(Record::Genesis(agreement)),
        AGREEMENT => parse_owned(agreement, rest),
        _ => None,
    }
}

fn parse_owned(agreement: AgreementId, key: &[u8]) -> Option<Record> {
    let (&collection, rest) = key.split_first()?;
    match collection {
        VIEW if rest.is_empty() => Some(Record::View(agreement)),
        CLOCK if rest.is_empty() => Some(Record::Clock(agreement)),
        ATTESTATION if rest.is_empty() => Some(Record::Attestation(agreement)),
        LOG => {
            let (author, rest) = take_id(rest)?;
            let seq = u64::from_be_bytes(rest.try_into().ok()?);

            Some(Record::Log {
                agreement,
                author,
                seq,
            })
        }
        ENTRY => {
            let (model, key) = take_id(rest)?;

            Some(Record::Entry {
                agreement,
                model: ModelId::from_bytes(model),
                key: key.to_vec(),
            })
        }
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
    use crate::agreement::Genesis;
    use crate::testing::peers::phone;

    fn id() -> AgreementId {
        Genesis::new([1; 32], [phone().id()]).id()
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
            parse(&entry(id(), model, b"color")),
            Some(Record::Entry {
                agreement: id(),
                model,
                key: b"color".to_vec()
            }),
        );
    }

    #[test]
    fn everything_an_agreement_owns_shares_its_prefix() {
        let prefix = agreement(id());
        let model = ModelId::new(id(), [2; 32]);

        assert!(view(id()).starts_with(&prefix));
        assert!(log(id(), phone(), 1).starts_with(&prefix));
        assert!(entry(id(), model, b"").starts_with(&prefix));
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
