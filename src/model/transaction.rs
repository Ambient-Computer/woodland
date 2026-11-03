use serde::{Deserialize, Serialize};

use super::{ModelId, Operation};
use crate::actor::{Peer, Signable};
use crate::agreement::AgreementId;
use crate::clock::Stamp;

/// An atomic group of operations against one model, and the unit that's
/// signed, sent, stored and integrated.
///
/// The transaction carries its own clock values:
///
/// - `seq` places it in its author's history. Each author's transactions
///   are numbered 1, 2, 3… and integrated in that order, which is what lets
///   a version vector summarize "everything up to here".
/// - `lamport` orders it against everyone else's for last-writer-wins.
///
/// The author isn't a field: it's whoever signed it. The agreement is a
/// field so a signed transaction can never be replayed into a different
/// agreement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Transaction {
    pub(crate) agreement: AgreementId,
    pub(crate) model: ModelId,
    pub(crate) seq: u64,
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
