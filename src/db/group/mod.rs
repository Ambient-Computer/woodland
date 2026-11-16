//! One group, as one device holds it: its checkpoint chain, everyone's
//! entries, and which of them have happened.

mod clock;
mod commit;
mod compact;
mod deferred;
mod forks;
mod history;
mod interest;
mod judge;
mod restore;
mod root;
mod rules;
mod sealing;
mod sync;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::actor::{Identity, Peer};
use crate::chain::{Certificate, Chain, Checkpoint, Digest, Electorate, Saved};
use crate::db::{ClockId, GroupId, Pending, keys};
use crate::model::Operation;
use crate::seal::Keyring;
use crate::store::{StoreError, View};

use super::proof::Proof;
use super::step::Step;
use clock::{Clock, Kept};
use compact::Staging;
pub(crate) use interest::Refused;
pub(crate) use sealing::found as found_sealed;
#[cfg(test)]
pub(crate) use sealing::{Sealing, about as sealed_about};
use sync::Remote;

#[derive(Debug)]
pub(crate) struct Group {
    id: GroupId,
    chain: Chain,
    /// Every clock anything has been written to, as far as this device
    /// has heard.
    clocks: BTreeMap<ClockId, Clock>,
    /// The height of the last checkpoint whose entries are committed
    /// here. Behind the chain's tip while those entries are on their way.
    folded: u64,
    /// The height below which history has been shed.
    base: u64,
    /// A snapshot that's part of the way in.
    staging: Option<Staging>,
    /// Proof against each contributor known to have forked.
    proofs: BTreeMap<Peer, Proof>,
    /// What each connected peer is thought to hold.
    remotes: BTreeMap<Peer, Remote>,
    /// The electorate this device has been asked to bring about.
    wanted: Option<Electorate>,
    /// Proposed checkpoints this device has checked against its own
    /// copy, and would vote for.
    sound: BTreeSet<Digest>,
    /// How many ticks the current attempt at a checkpoint has gone on.
    stalled: u64,
    /// Whether the chain has anything peers haven't been told.
    said: bool,
    /// The clocks this device wants. `None` if all of them.
    interest: Option<BTreeSet<ClockId>>,
    /// Whether a checkpoint has been committed here without the state
    /// of a clock it covers, because the clock wasn't wanted.
    passed: bool,
    /// Whether this device is getting back checkpoints it shed.
    mending: bool,
    /// What this device can read of the group.
    keys: Keyring,
}

/// What's kept of a group besides its chain and its entries.
#[derive(Debug, Serialize, Deserialize)]
struct Meta {
    clocks: BTreeMap<ClockId, Kept>,
    folded: u64,
    base: u64,
    staging: Option<Staging>,
    wanted: Option<Electorate>,
    interest: Option<BTreeSet<ClockId>>,
    passed: bool,
    mending: bool,
    keys: Keyring,
}

/// What came of trying to write.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Wrote {
    Yes,
    /// This device isn't a contributor to the clock.
    NotAllowed,
    /// It can't read the clock, so it doesn't know what it would be
    /// writing on top of.
    Unreadable,
}

/// What one device hands another so it can hold a group.
///
/// The founding names the group. The anchor is the latest checkpoint
/// the inviter held, with proof that it follows from the founding. The
/// newcomer takes the group to be at least that far along, on the
/// inviter's word and the proof's, and nothing that doesn't follow from
/// the anchor can move it: not even the keys that founded the group,
/// if they've since been removed, lost, or stolen.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Invitation {
    founding: Checkpoint,
    anchor: Vec<Certificate>,
}

impl Invitation {
    /// The chain it describes, if its proof holds.
    pub(crate) fn chain(self) -> Option<Chain> {
        let genuine = self.founding.height == 0;

        Chain::anchored(self.founding, self.anchor).filter(|_| genuine)
    }
}

/// A group is named by the checkpoint it starts from.
pub(crate) fn id_of(founding: &Checkpoint) -> GroupId {
    GroupId::from_bytes(*founding.digest().as_bytes())
}

impl Group {
    fn new(id: GroupId, chain: Chain) -> Self {
        let founding = chain.checkpoints().next();
        let keys = founding.map(sealing::keyring_of).unwrap_or_default();

        Self {
            id,
            chain,
            clocks: BTreeMap::new(),
            folded: 0,
            base: 0,
            staging: None,
            proofs: BTreeMap::new(),
            remotes: BTreeMap::new(),
            wanted: None,
            sound: BTreeSet::new(),
            stalled: 0,
            said: false,
            interest: None,
            passed: false,
            mending: false,
            keys,
        }
    }

    /// Starts holding a group, from as much of its chain as this
    /// device was given.
    pub(crate) fn start<V: View>(chain: Chain, step: &mut Step<V>) -> Option<Self> {
        let founding = chain.checkpoints().next()?;
        let id = id_of(founding);
        step.save(keys::founding(id), founding);
        let mut group = Self::new(id, chain);
        group.said = true;
        group.persist(step);

        Some(group)
    }

    /// What another device needs to start holding this group: where it
    /// began, and proof of where it has got to.
    pub(crate) fn invitation(&self) -> Option<Invitation> {
        Some(Invitation {
            founding: self.chain.checkpoints().next()?.clone(),
            anchor: self.chain.anchor(),
        })
    }

    /// Picks a group up from storage.
    pub(crate) async fn load<V: View>(
        id: GroupId,
        step: &Step<V>,
    ) -> Result<Option<Self>, StoreError> {
        let saved = step.load::<Saved>(&keys::chain(id)).await?;
        let meta = step.load::<Meta>(&keys::meta(id)).await?;
        let (Some(saved), Some(meta)) = (saved, meta) else {
            return Ok(None);
        };

        let mut group = Self::new(id, Chain::restore(saved));
        group.folded = meta.folded;
        group.base = meta.base;
        group.staging = meta.staging;
        group.wanted = meta.wanted;
        group.interest = meta.interest;
        group.passed = meta.passed;
        group.mending = meta.mending;
        group.keys = meta.keys;
        group.recall(step).await?;
        for (clock, kept) in meta.clocks {
            let held = Clock::load(id, clock, kept, group.keys.clone(), step).await?;
            group.clocks.insert(clock, held);
        }

        Ok(Some(group))
    }

    /// The height of the last checkpoint committed here.
    #[cfg(test)]
    pub(crate) fn folded(&self) -> u64 {
        self.folded
    }

    /// Every change waiting to be committed, clock by clock.
    pub(crate) async fn pending<V: View>(
        &self,
        step: &Step<V>,
    ) -> Result<Vec<Pending>, StoreError> {
        let mut pending = Vec::new();
        for clock in self.clocks.values() {
            pending.extend(clock.pending(step).await?);
        }

        Ok(pending)
    }

    /// Whether `peer` has a role in the group, as far as this device
    /// knows.
    pub(crate) fn has(&self, peer: Peer) -> bool {
        self.chain.tip().electorate.has(peer)
    }

    /// How many checkpoints it holds, the founding one included.
    #[cfg(test)]
    pub(crate) fn checkpoints(&self) -> usize {
        self.chain.checkpoints().count()
    }

    /// Every checkpoint it holds at `height` or below, with the votes
    /// that decided it, latest first.
    #[cfg(test)]
    pub(crate) fn links_below(&self, height: u64) -> Vec<Certificate> {
        self.chain
            .older(height.min(self.chain.tip().height), usize::MAX)
    }

    /// Whether anyone it has heard from, itself included, is trying
    /// for the next checkpoint.
    #[cfg(test)]
    pub(crate) fn attempting(&self) -> bool {
        self.chain.attempt().is_some()
    }

    /// How many checkpoints it holds proposals of, for the next place
    /// in the chain.
    #[cfg(test)]
    pub(crate) fn proposals(&self) -> usize {
        self.chain.proposed().count()
    }

    /// The latest checkpoint this device knows to be final.
    #[cfg(test)]
    pub(crate) fn tip(&self) -> &Checkpoint {
        self.chain.tip()
    }

    /// Whether anyone here has been caught going back on its word
    /// about a checkpoint.
    #[cfg(test)]
    pub(crate) fn caught_a_conflict(&self) -> bool {
        let conflict = |proof: &Proof| matches!(proof, Proof::Conflict(_));

        self.proofs.values().any(conflict)
    }

    fn persist<V: View>(&self, step: &mut Step<V>) {
        let clocks = self.clocks.iter();
        let meta = Meta {
            clocks: clocks.map(|(&id, clock)| (id, clock.kept())).collect(),
            folded: self.folded,
            base: self.base,
            staging: self.staging.clone(),
            wanted: self.wanted.clone(),
            interest: self.interest.clone(),
            passed: self.passed,
            mending: self.mending,
            keys: self.keys.clone(),
        };
        step.save(keys::meta(self.id), &meta);
        if self.said {
            step.save(keys::chain(self.id), &self.chain.save());
        }
    }

    /// Appends an entry to this device's own chain in `clock`, if it
    /// may: only a contributor to that clock writes, and only to a
    /// clock it can read.
    pub(crate) async fn write<V: View>(
        &mut self,
        identity: &Identity,
        clock: ClockId,
        operations: Vec<Operation>,
        step: &mut Step<V>,
    ) -> Result<Wrote, StoreError> {
        let tip = self.chain.tip().clone();
        // A device that learns it forked has lost track of what it
        // signed, and anything more it wrote would be dropped unread.
        let me = identity.peer();
        let may = tip.electorate.writers(clock).includes(me) && self.wants(clock);
        if !may || self.proofs.contains_key(&me) {
            return Ok(Wrote::NotAllowed);
        }
        if !self.clock(clock).write(identity, &tip, operations, step) {
            return Ok(Wrote::Unreadable);
        }

        self.progress(identity, None, step).await?;

        Ok(Wrote::Yes)
    }

    /// The clock named `id`, which exists from the moment it's needed.
    fn clock(&mut self, id: ClockId) -> &mut Clock {
        let group = self.id;

        let keys = &self.keys;
        self.clocks.entry(id).or_insert_with(|| {
            let mut clock = Clock::new(group, id);
            clock.keys = keys.clone();
            clock
        })
    }

    /// Asks for `electorate` to decide checkpoints from the next one on.
    /// Returns whether this device has a say.
    pub(crate) async fn reconfigure<V: View>(
        &mut self,
        identity: &Identity,
        electorate: Electorate,
        step: &mut Step<V>,
    ) -> Result<bool, StoreError> {
        let allowed = self.chain.tip().electorate.includes(identity.peer());
        let sealable = !self.is_sealed() || electorate.is_sealable();
        if !allowed || !electorate.is_viable() || !sealable {
            return Ok(false);
        }

        self.wanted = Some(electorate);
        self.progress(identity, None, step).await?;

        Ok(true)
    }

    /// Sheds history that's committed. Reads are the same afterwards.
    pub(crate) async fn shed<V: View>(
        &mut self,
        identity: &Identity,
        only: Option<ClockId>,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        self.compact(only, step).await?;

        self.progress(identity, None, step).await
    }

    /// Time has passed. It's when a checkpoint is attempted, and when a
    /// stalled attempt is retried.
    pub(crate) async fn tick<V: View>(
        &mut self,
        identity: &Identity,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        if self.chain.attempt().is_some() {
            self.stalled += 1;
            self.said = true;
        } else {
            self.stalled = 0;
        }

        for remote in self.remotes.values_mut() {
            remote.doubt();
        }

        self.propose(identity, step).await?;
        self.progress(identity, None, step).await
    }

    /// Does everything that follows from what's now held: decides and
    /// commits what it can, works out what happens, and tells peers.
    /// `owed` is a peer that gets an answer even if there's no news.
    async fn progress<V: View>(
        &mut self,
        identity: &Identity,
        owed: Option<Peer>,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        loop {
            self.unlock(identity, step).await?;
            self.vet(identity, step).await?;
            let moved = self.advance(identity);
            self.said |= moved;
            let folded = self.fold(step).await?;
            if !moved && !folded {
                break;
            }
        }

        self.clocks.retain(|_, clock| !clock.is_empty());
        for clock in self.clocks.values_mut() {
            clock.rejudge(step).await?;
        }

        self.persist(step);
        let peers: Vec<Peer> = self.remotes.keys().copied().collect();
        for peer in peers {
            self.push(peer, owed == Some(peer), self.said, step).await?;
        }
        self.said = false;

        Ok(())
    }
}
