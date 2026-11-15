//! What peers say to each other.

use serde::{Deserialize, Serialize};

use crate::chain::Gossip;
use crate::db::GroupId;

use super::entry::{Entry, Frontier};

/// Everything one peer tells another about one group, at once. Every
/// part is safe to hear twice, late, or from someone else.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Message {
    pub(crate) group: GroupId,
    pub(crate) summary: Summary,
    pub(crate) chain: Gossip,
    pub(crate) entries: Vec<Entry>,
}

/// How much a peer holds, so the other side can work out what it lacks.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Summary {
    /// The height of its latest checkpoint.
    pub(crate) height: u64,
    /// The last entry it holds of each author's chain.
    pub(crate) heads: Frontier,
}
