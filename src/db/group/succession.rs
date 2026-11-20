//! A group that carries on from another.
//!
//! A group that has lost its quorum for good can't commit again, and
//! nothing inside it can change that. Whoever is left founds another
//! one that starts where it stopped: the new group's first checkpoint
//! names the old group's last, and what it commits to is that
//! checkpoint's state.
//!
//! To the new group, that state is a snapshot at its founding. The
//! device that founds it takes the snapshot from the old group as it
//! holds it, a piece per step, and checks it like any other. Devices
//! that join later take it from a peer.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::actor::Identity;
use crate::chain::Digest;
use crate::db::{GroupId, keys};
use crate::encoding;
use crate::seal::{Keyring, Secret};
use crate::store::{StoreError, View};

use super::super::step::Step;
use super::Group;
use super::commit::{Covered, Covers, covers_of};
use super::compact::Sending;
use super::root::Accumulator;
use super::sealing::found_with;

/// Whether a device has the state its group starts from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Origin {
    /// It has: the group started from nothing, or the state is in.
    #[default]
    Whole,
    /// It hasn't, and waits to be sent it.
    Awaited,
    /// It hasn't, and is bringing it over from the group this one
    /// carries on from, which it holds.
    Inherited,
}

/// The checkpoint of another group that a group starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Succeeds {
    pub(crate) group: GroupId,
    pub(crate) checkpoint: Digest,
}

impl Group {
    /// The checkpoint this group started from, if it carries on from
    /// another.
    pub(crate) fn succeeds(&self) -> Option<Succeeds> {
        let founding = self.chain.checkpoints().next()?;

        covers_of(founding)?.succeeds
    }

    /// The checkpoint a successor founded now would name.
    fn last(&self) -> Option<Digest> {
        Some(self.chain.at(self.folded)?.digest())
    }

    /// What the founding of a group that carries on from this one says:
    /// where this one stopped, and what that checkpoint covered. Each
    /// author's chain carries on from where it had got to, as it does
    /// for a device that takes a snapshot. The successor of a sealed
    /// group is sealed, under a secret of its own made from `seed`,
    /// which comes back too. `None` if this device doesn't hold all of
    /// that state, or can't read it.
    pub(crate) async fn bequest<V: View>(
        &self,
        identity: &Identity,
        seed: [u8; 32],
        step: &Step<V>,
    ) -> Result<Option<(Option<Secret>, Vec<u8>)>, StoreError> {
        let whole = !self.is_partway() && !self.partial() && !self.is_unfounded();
        let readable = self.reads() && !self.is_blind();
        let last = self.chain.at(self.folded).filter(|_| whole && readable);
        let Some((last, covered)) = last.and_then(|it| Some((it, covers_of(it)?))) else {
            return Ok(None);
        };

        // A state is named under a secret of the group it's in, so the
        // same state has another name there.
        let (secret, keys, mut covers) = if self.is_sealed() {
            let (secret, keys, covers) = found_with(identity, seed);
            (Some(secret), keys, covers)
        } else {
            (None, Keyring::Open, Covers::default())
        };
        for (id, covered) in covered.clocks {
            let sum = step.get(&keys::state(self.id, id)).await?;
            let sum = sum.and_then(|bytes| Accumulator::from_bytes(&bytes));
            let state = sum.unwrap_or_default().root(&keys);
            let frontier = covered.frontier;
            covers.clocks.insert(id, Covered { frontier, state });
        }
        covers.succeeds = Some(Succeeds {
            group: self.id,
            checkpoint: last.digest(),
        });

        Ok(Some((secret, encoding::encode(&covers))))
    }

    /// Whether this device lacks the state the group starts from.
    pub(super) fn is_unfounded(&self) -> bool {
        self.origin != Origin::Whole
    }

    /// Has the group be as it is with nothing held of it: on starting
    /// to hold it, or on throwing away everything held. If it starts
    /// from state, that's still to come, but where each author's chain
    /// had got to is known already, and what's written goes after.
    pub(super) fn originate<V: View>(&mut self, step: &mut Step<V>) {
        let founding = self.chain.checkpoints().next();
        let covers = founding.and_then(covers_of).unwrap_or_default();
        self.origin = Origin::Whole;
        for (&id, covered) in &covers.clocks {
            self.origin = Origin::Awaited;
            self.clock(id)
                .adopt(&covered.frontier, BTreeSet::new(), step);
        }
    }

    /// Whether this device founded the group, and is still bringing
    /// over the state it starts from.
    pub(crate) fn is_inheriting(&self) -> bool {
        self.origin == Origin::Inherited
    }

    /// Holds this group's committed state where it is while `heir`
    /// copies it, or lets it go on.
    pub(crate) fn pin(&mut self, heir: Option<GroupId>) {
        self.pinned = heir;
    }

    /// Whether a group founded to carry on from this one is still
    /// copying its state.
    pub(crate) fn is_pinned(&self) -> bool {
        self.pinned.is_some()
    }

    /// Has the group take its first state from the one it carries on
    /// from, which this device holds.
    pub(crate) fn inherit<V: View>(&mut self, step: &mut Step<V>) {
        if self.is_unfounded() {
            self.origin = Origin::Inherited;
        }
        self.persist(step);
    }

    /// Takes the next piece of the state this group starts from, out
    /// of `old`: the group it carries on from.
    pub(crate) async fn inherit_some<V: View>(
        &mut self,
        old: &Self,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let named = self.succeeds().map(|it| it.checkpoint);
        if old.last() != named || named.is_none() {
            tracing::warn!(group = ?self.id, "the group this one carries on from has moved on");
            self.origin = Origin::Awaited;
            self.persist(step);

            return Ok(());
        }
        if self.installing.is_some() {
            return Ok(());
        }

        // The old group is the sender, and says its own height. Here
        // it's the state of the founding.
        let staged = self.staging.as_ref();
        self.heir.staged = staged.map(|it| (old.folded, it.upto.clone()));
        let named = self.is_sealed();
        let Some((mut piece, at)) = old.plain_piece(&self.heir, named, step).await? else {
            return Ok(());
        };
        piece.height = 0;
        let done = piece.end.is_some();
        let Some(piece) = self.seal_piece(piece) else {
            return Ok(());
        };
        self.heir.sending = Some(Sending {
            height: old.folded,
            upto: piece.upto.clone(),
            at,
            done,
        });

        self.stage(piece, step).await
    }
}
