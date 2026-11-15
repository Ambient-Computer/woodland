//! What peers say to each other.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::actor::Peer;
use crate::agreement::AgreementId;
use crate::chain::Gossip;

use super::entry::Entry;

/// Everything one peer tells another about one group, at once. Every
/// part is safe to hear twice, late, or from someone else.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Message {
    pub(crate) group: AgreementId,
    pub(crate) summary: Summary,
    pub(crate) chain: Gossip,
    pub(crate) entries: Vec<Entry>,
}

/// How much a peer holds, so the other side can work out what it lacks.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Summary {
    /// The height of its latest checkpoint.
    pub(crate) height: u64,
    /// How far along each author's chain it holds.
    pub(crate) heads: BTreeMap<Peer, u64>,
}
