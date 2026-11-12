use serde::{Deserialize, Serialize};

use crate::actor::{Peer, Signable};
use crate::agreement::AgreementId;
use crate::clock::{Stamp, VersionVector};
use crate::model::Operation;

/// An atomic group of operations, against any of the agreement's models,
/// and the unit that's
/// signed, sent, stored and integrated.
///
/// The transaction carries its own clock values:
///
/// - `seq` places it in its author's history. Each author's transactions
///   are numbered 1, 2, 3… and integrated in that order, which is what lets
///   a version vector summarize "everything up to here".
/// - `after` is what its author had integrated when writing it: the
///   transactions it was written on top of. A transaction that's in neither
///   one's `after` is concurrent with the other. Nobody integrates a
///   transaction before everything in its `after`, so every device's
///   history is one its authors could have seen (causal delivery). The
///   author's own earlier transactions are implied by `seq`.
/// - `lamport` orders it against everyone else's for last-writer-wins. It's
///   greater than that of everything in `after`, so sorting by it never
///   puts a transaction before one it was written on top of.
///
/// The author isn't a field: it's whoever signed it. The agreement is a
/// field so a signed transaction can never be replayed into a different
/// agreement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Transaction {
    pub(crate) agreement: AgreementId,
    pub(crate) seq: u64,
    pub(crate) after: VersionVector,
    pub(crate) lamport: u64,
    pub(crate) operations: Vec<Operation>,
}

impl Transaction {
    pub(crate) fn stamp(&self, author: Peer) -> Stamp {
        Stamp {
            lamport: self.lamport,
            author,
        }
    }
}

impl Signable for Transaction {
    const CONTEXT: &'static [u8] = b"woodland/transaction";
}
