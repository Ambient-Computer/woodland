//! What peers say to each other.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::chain::Gossip;
use crate::db::{ClockId, GroupId};
use crate::model::ModelId;

use crate::actor::Peer;

use super::entry::{Entry, Frontier};
use super::proof::Proof;

/// Everything one peer tells another about one group, at once. Every
/// part is safe to hear twice, late, or from someone else.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Message {
    pub(crate) group: GroupId,
    pub(crate) summary: Summary,
    pub(crate) chain: Gossip,
    pub(crate) entries: Vec<Entry>,
    /// Fork proofs the other side hasn't said it holds.
    pub(crate) proofs: Vec<Proof>,
    pub(crate) snapshot: Option<Piece>,
}

/// How much a peer holds, so the other side can work out what it lacks.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Summary {
    /// The height of its latest checkpoint.
    pub(crate) height: u64,
    /// The height of the latest checkpoint it has committed.
    pub(crate) folded: u64,
    /// Whether it can't commit its way forward entry by entry: it
    /// doesn't hold the checkpoint after the last one it committed.
    /// It joined partway along, and needs the state instead.
    pub(crate) gap: bool,
    /// The snapshot it's part of the way through receiving: the height
    /// it's for, and the last key it has.
    pub(crate) staged: Option<(u64, Option<Vec<u8>>)>,
    /// The last entry it holds of each author's chain, in each clock.
    pub(crate) heads: BTreeMap<ClockId, Frontier>,
    /// Everyone it holds a fork proof against.
    pub(crate) proven: BTreeSet<Peer>,
}

/// Part of a snapshot: committed values, in key order.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Piece {
    /// The checkpoint this is the state of.
    pub(crate) height: u64,
    /// The key this piece comes after. `None` for the first.
    pub(crate) start: Option<Vec<u8>>,
    /// Each key is a model and then the model's own key, so a clock's
    /// values come together.
    pub(crate) values: Vec<(Vec<u8>, Vec<u8>)>,
    /// With the last piece: every model that's been dropped.
    pub(crate) end: Option<BTreeSet<ModelId>>,
}
