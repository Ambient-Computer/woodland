use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::actor::Peer;

/// Who has a say in the next checkpoint, and how much each one counts.
///
/// Weights are the application's choice. A device that's always on can
/// outweigh the rest, so that it and any one other are enough. A peer
/// with more than half the weight decides alone, and equal weights with
/// everyone needed is unanimity. None of those is a special case.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Electorate(BTreeMap<Peer, u32>);

impl Electorate {
    /// A peer with no weight has no say, so it isn't kept.
    pub(crate) fn new(weights: impl IntoIterator<Item = (Peer, u32)>) -> Self {
        Self(
            weights
                .into_iter()
                .filter(|(_, weight)| *weight > 0)
                .collect(),
        )
    }

    pub(crate) fn includes(&self, peer: Peer) -> bool {
        self.0.contains_key(&peer)
    }

    /// Nobody could ever extend a checkpoint that carries this.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether `holders` carry more than half the total weight.
    ///
    /// Any two such sets share a member. That overlap is what agreement's
    /// safety rests on: the shared member has seen both sides.
    pub(crate) fn has_quorum(&self, holders: &BTreeSet<Peer>) -> bool {
        let weight = |peer: &Peer| u64::from(self.0.get(peer).copied().unwrap_or(0));
        let total: u64 = self.0.keys().map(weight).sum();
        let held: u64 = holders.iter().map(weight).sum();

        held * 2 > total
    }
}
