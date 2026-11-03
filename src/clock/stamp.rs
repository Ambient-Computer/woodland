use serde::{Deserialize, Serialize};

use crate::actor::Peer;

/// When a transaction happened, as a total order every device agrees on.
///
/// The Lamport counter respects causality: a transaction's counter is
/// greater than that of everything its author had seen. Concurrent
/// transactions are ordered arbitrarily but deterministically by author.
///
/// Field order matters: the derived `Ord` compares `lamport` first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct Stamp {
    pub(crate) lamport: u64,
    pub(crate) author: Peer,
}
