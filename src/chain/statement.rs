//! What electors say to each other, and the checkpoint they say it about.
//!
//! Every statement is signed by whoever made it and names the slot it's
//! about, so it means the same thing whoever passes it on, and can never
//! be replayed into another slot or another chain.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::Electorate;
use crate::actor::{Peer, Signable, Signed, Verified, verify_all};
use crate::encoding;

/// Names a checkpoint by its content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct Digest([u8; 32]);

impl Digest {
    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// One link of the chain.
///
/// It names the link before it, so the chain is a sequence: whoever holds
/// a checkpoint knows exactly which history it closes. What it makes
/// final is opaque here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Checkpoint {
    pub(crate) previous: Digest,
    pub(crate) height: u64,
    /// Who decides the checkpoint after this one.
    pub(crate) electorate: Electorate,
    pub(crate) body: Vec<u8>,
}

impl Checkpoint {
    /// The start of a chain. `seed` is what no other founding shares.
    pub(crate) fn founding(seed: [u8; 32], electorate: Electorate) -> Self {
        Self {
            previous: Digest(seed),
            height: 0,
            electorate,
            body: Vec::new(),
        }
    }

    pub(crate) fn digest(&self) -> Digest {
        Digest(encoding::hash("woodland/checkpoint", self))
    }

    /// A candidate for the link after this one.
    pub(crate) fn extend(&self, electorate: Electorate, body: Vec<u8>) -> Self {
        Self {
            previous: self.digest(),
            height: self.height + 1,
            electorate,
            body,
        }
    }

    pub(crate) fn extends(&self, tip: &Self) -> bool {
        self.previous == tip.digest() && self.height == tip.height + 1
    }
}

/// One attempt to fill a slot. A later attempt outranks an earlier one,
/// and the proposer breaks ties, so no two attempts are ever equal.
///
/// Field order matters: the derived `Ord` compares `round` first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct Ballot {
    pub(crate) round: u64,
    pub(crate) proposer: Peer,
}

/// A statement about one attempt at one slot. The slot is named by the
/// checkpoint it would extend.
pub(crate) trait Stated {
    fn slot(&self) -> Digest;
    fn ballot(&self) -> Ballot;
}

/// "I'd like to fill this slot. Tell me what you've already voted for."
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Prepare {
    pub(crate) slot: Digest,
    pub(crate) ballot: Ballot,
}

/// "I won't vote in any earlier attempt. Here's my latest vote, if any."
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Promise {
    pub(crate) slot: Digest,
    pub(crate) ballot: Ballot,
    pub(crate) accepted: Option<(Ballot, Checkpoint)>,
}

/// "A quorum promised. This is the checkpoint to vote for."
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Proposal {
    pub(crate) slot: Digest,
    pub(crate) ballot: Ballot,
    pub(crate) checkpoint: Checkpoint,
}

/// "I vote for this checkpoint, in this attempt."
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Vote {
    pub(crate) slot: Digest,
    pub(crate) ballot: Ballot,
    pub(crate) checkpoint: Digest,
}

macro_rules! stated {
    ($($statement:ident: $context:literal),* $(,)?) => {$(
        impl Signable for $statement {
            const CONTEXT: &'static [u8] = $context;
        }

        impl Stated for $statement {
            fn slot(&self) -> Digest {
                self.slot
            }

            fn ballot(&self) -> Ballot {
                self.ballot
            }
        }
    )*};
}

stated! {
    Prepare: b"woodland/chain/prepare",
    Promise: b"woodland/chain/promise",
    Proposal: b"woodland/chain/proposal",
    Vote: b"woodland/chain/vote",
}

/// A checkpoint, and the votes that made it final.
///
/// It proves itself to anyone who holds the checkpoint before it, since
/// that one says whose votes count. So a chain can be handed to a device
/// that was away for all of it, by a device that had no say in any of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Certificate {
    pub(crate) checkpoint: Checkpoint,
    pub(crate) ballot: Ballot,
    pub(crate) votes: Vec<Signed<Vote>>,
}

impl Certificate {
    /// Whether a quorum of `tip`'s electorate voted for this checkpoint,
    /// in one attempt, as the link after `tip`.
    pub(crate) fn proves(&self, tip: &Checkpoint) -> bool {
        self.checkpoint.extends(tip) && self.voted(tip)
    }

    /// Whether a quorum of `tip`'s electorate voted for this checkpoint,
    /// in one attempt, somewhere further along the chain.
    ///
    /// The links between may be missing: a device that sheds history
    /// keeps only the checkpoints that changed the electorate, and the
    /// latest. An electorate that hasn't changed since `tip` is still the
    /// one whose votes count, so its quorum is proof enough, and a quorum
    /// only ever votes for a checkpoint that extends its own chain.
    pub(crate) fn proves_beyond(&self, tip: &Checkpoint) -> bool {
        self.checkpoint.height > tip.height && self.voted(tip)
    }

    fn voted(&self, tip: &Checkpoint) -> bool {
        let expected = Vote {
            slot: self.checkpoint.previous,
            ballot: self.ballot,
            checkpoint: self.checkpoint.digest(),
        };
        let voters: BTreeSet<Peer> = verify_all(self.votes.clone())
            .iter()
            .filter(|vote| *vote.payload() == expected)
            .map(Verified::author)
            .collect();

        tip.electorate.has_quorum(&voters)
    }
}
