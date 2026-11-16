//! What peers say to each other.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::actor::{Signable, Signed};
use crate::chain::{Certificate, Electorate, Gossip};
use crate::db::{ClockId, GroupId};
use crate::model::ModelId;

use crate::actor::Peer;

use super::entry::{Entry, Frontier};
use super::group::Handed;
use super::proof::Proof;

/// Everything one peer tells another about one group, at once. Every
/// part is safe to hear twice, late, or from someone else.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Message {
    pub(crate) group: GroupId,
    pub(crate) summary: Summary,
    pub(crate) chain: Gossip,
    pub(crate) entries: Vec<Entry>,
    /// Checkpoints the receiver shed and asked to have back, latest
    /// first.
    pub(crate) links: Vec<Certificate>,
    /// History the receiver shed and asked to have back: entries a
    /// checkpoint covers, latest first in each chain.
    pub(crate) restored: Vec<Entry>,
    /// Fork proofs the other side hasn't said it holds.
    pub(crate) proofs: Vec<Proof>,
    pub(crate) snapshot: Option<Piece>,
    /// Requests to change who's in the group that the other side
    /// hasn't been told of.
    pub(crate) requests: Vec<Signed<Request>>,
    /// A secret the peer said it lacks and is due.
    pub(crate) secret: Option<Handed>,
}

/// A member's request that the group be made up differently. It's
/// made of the group as it is: once that changes, it's spent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Request {
    pub(crate) group: GroupId,
    /// The height of the latest checkpoint when it was made.
    pub(crate) since: u64,
    pub(crate) electorate: Electorate,
}

impl Signable for Request {
    const CONTEXT: &'static [u8] = b"woodland/request";
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
    /// The clocks it wants anything of. `None` if all.
    pub(crate) interest: Option<BTreeSet<ClockId>>,
    /// The entry of each chain it shed and wants back next.
    pub(crate) restoring: BTreeMap<ClockId, Frontier>,
    /// The height of the checkpoint it shed and wants back next.
    pub(crate) lacking: Option<u64>,
    /// The secret it lacks and wants handed to it: the one brought in
    /// by the checkpoint at this height.
    pub(crate) wanting: Option<u64>,
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
    /// The last key this piece gets to, whether or not it's among the
    /// values: those of clocks the receiver doesn't want are left out.
    pub(crate) upto: Option<Vec<u8>>,
    /// With the last piece: every model that's been dropped.
    pub(crate) end: Option<BTreeSet<ModelId>>,
    /// In a sealed group, all of the above that says anything, as
    /// only its readers can read it.
    pub(crate) sealed: Option<SealedPiece>,
}

/// A piece of a sealed group's snapshot. Its values and what the
/// state has dropped are encrypted, and where it starts and ends are
/// names that mean something only to whoever made it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SealedPiece {
    pub(crate) body: Vec<u8>,
    /// Whether it's the last piece.
    pub(crate) last: bool,
}
