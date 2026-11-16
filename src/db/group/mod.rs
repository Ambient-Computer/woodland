//! One group, as one device holds it: its checkpoint chain, everyone's
//! entries, and which of them have happened.

mod ahead;
mod carry;
mod clock;
mod commit;
mod compact;
mod deferred;
mod foreign;
mod forks;
mod history;
mod indexes;
mod interest;
mod job;
mod judge;
mod requests;
mod restore;
mod root;
mod rules;
mod sealing;
mod succession;
mod sync;
mod verdicts;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::actor::{Identity, Peer, Signed, verify_all};
use crate::chain::{Certificate, Chain, Checkpoint, Digest, Electorate, Saved};
use crate::db::{ClockId, GroupId, Pending, Reached, keys};
use crate::encoding;
use crate::model::Operation;
use crate::seal::Keyring;
use crate::store::{StoreError, View};

use super::message::Request;
use super::proof::Proof;
use super::step::Step;
use ahead::Foresight;
use clock::{Clock, Kept};
use compact::{Installing, Staging};
pub(crate) use interest::Refused;
pub(crate) use sealing::Handed;
pub(crate) use sealing::found as found_sealed;
#[cfg(test)]
pub(crate) use sealing::{Sealing, about as sealed_about};
use succession::Origin;
use sync::Remote;

#[cfg(test)]
pub(crate) use commit::STRIDE;
#[cfg(test)]
pub(crate) use compact::SWEEP;

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
    /// What members have asked the group to be made up of, this
    /// device included.
    requests: BTreeMap<Peer, Signed<Request>>,
    /// The indexes this device keeps of the group's tables.
    indexes: indexes::Indexes,
    /// Whether this device has the state the group starts from, if
    /// it carries on from another group.
    origin: Origin,
    /// Where it has got to, as a peer being sent a snapshot.
    heir: Remote,
    /// The group on this device that's still bringing its first state
    /// over from this one. Until it has, nothing more is committed
    /// here: the state it names has to stay where it is.
    pinned: Option<GroupId>,
    /// The heights of the decided checkpoints that are in storage.
    stored: BTreeSet<u64>,
    /// Proposed checkpoints this device has checked against its own
    /// copy, and would vote for.
    sound: BTreeSet<Digest>,
    /// What it's working out ahead of a decision.
    fore: Foresight,
    /// Makes it write entries under a checkpoint nobody decided, if
    /// it isn't empty.
    #[cfg(test)]
    pub(crate) misnaming: Vec<u8>,
    /// How many ticks the current attempt at a checkpoint has gone on.
    stalled: u64,
    /// How far the attempt had got at the last tick.
    gathered: (u64, usize),
    /// The height this device last tried to follow, and how many
    /// times it's tried.
    tried: (u64, u32),
    /// Whether the chain has anything peers haven't been told.
    said: bool,
    /// The clocks this device wants. `None` if all of them.
    interest: Option<BTreeSet<ClockId>>,
    /// Whether a checkpoint has been committed here without the state
    /// of a clock it covers, because the clock wasn't wanted.
    passed: bool,
    /// The height of a checkpoint that's partly committed. Until the
    /// rest is, committed state is the state of no checkpoint.
    folding: Option<u64>,
    /// A whole snapshot that's partly in place. Until the rest is,
    /// committed state is some of each.
    installing: Option<Installing>,
    /// Whether this device is getting back checkpoints it shed.
    mending: bool,
    /// What this device can read of the group.
    keys: Keyring,
    /// The height of the sealed snapshot this device keeps without
    /// being able to read it.
    carrying: Option<u64>,
    /// The secret this device wants replaced: the one brought in by
    /// the checkpoint at this height. Once another is in force, it's
    /// done.
    renewing: Option<u64>,
    /// The height this device had committed to when it asked to be
    /// sent state rather than history. Once it's past that and has
    /// caught up, it's done.
    skipping: Option<u64>,
    /// How many kinds of model the library had when this was last
    /// saved.
    spoke: u8,
    /// The kinds the application had added when this was opened.
    extended: BTreeSet<u8>,
}

/// What's kept of a group besides its chain and its entries.
#[derive(Debug, Serialize, Deserialize)]
struct Meta {
    clocks: BTreeMap<ClockId, Kept>,
    folded: u64,
    base: u64,
    staging: Option<Staging>,
    wanted: Option<Electorate>,
    requests: Vec<Signed<Request>>,
    origin: Origin,
    interest: Option<BTreeSet<ClockId>>,
    passed: bool,
    folding: Option<u64>,
    installing: Option<Installing>,
    littered: bool,
    mending: bool,
    keys: Keyring,
    carrying: Option<u64>,
    renewing: Option<u64>,
    skipping: Option<u64>,
    /// How many kinds of model the library had when this was saved.
    spoke: u8,
    /// The kinds the application had added.
    extended: BTreeSet<u8>,
}

/// What came of trying to write.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Wrote {
    /// As the entry at this place in its chain.
    Yes(u64),
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
            requests: BTreeMap::new(),
            indexes: BTreeMap::new(),
            origin: Origin::Whole,
            heir: Remote::default(),
            pinned: None,
            stored: BTreeSet::new(),
            sound: BTreeSet::new(),
            fore: Foresight::default(),
            #[cfg(test)]
            misnaming: Vec::new(),
            stalled: 0,
            gathered: (0, 0),
            tried: (0, 0),
            said: false,
            interest: None,
            passed: false,
            folding: None,
            installing: None,
            mending: false,
            keys,
            carrying: None,
            renewing: None,
            skipping: None,
            spoke: crate::model::vocabulary(),
            extended: crate::model::extension::numbers(),
        }
    }

    /// Starts holding a group, from as much of its chain as this
    /// device was given.
    pub(crate) fn start<V: View>(chain: Chain, step: &mut Step<V>) -> Option<Self> {
        let founding = chain.checkpoints().next()?;
        let id = id_of(founding);
        step.save(keys::founding(id), founding);
        let mut group = Self::new(id, chain);
        group.originate(step);
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

        let (start, end) = keys::range(keys::links(id));
        let mut links = Vec::new();
        for (_, link) in step.scan(&start, &end).await? {
            let link = encoding::decode::<Certificate>(&link);
            links.push(link.ok_or_else(|| StoreError::new("a stored checkpoint is unreadable"))?);
        }
        let stored = links.iter().map(|link| link.checkpoint.height).collect();
        let mut group = Self::new(id, Chain::restore(saved, links));
        group.stored = stored;
        group.folded = meta.folded;
        group.base = meta.base;
        group.staging = meta.staging;
        group.wanted = meta.wanted;
        group.origin = meta.origin;
        let requests = verify_all(meta.requests).into_iter();
        group.requests = requests
            .map(|it| (it.author(), it.signed().clone()))
            .collect();
        group.interest = meta.interest;
        group.passed = meta.passed;
        group.folding = meta.folding;
        group.fore.littered = meta.littered;
        group.installing = meta.installing;
        group.mending = meta.mending;
        group.keys = meta.keys;
        group.carrying = meta.carrying;
        group.renewing = meta.renewing;
        group.skipping = meta.skipping;
        group.spoke = meta.spoke;
        group.extended = meta.extended;
        group.recall(step).await?;
        group.recall_indexes(step).await?;
        for (clock, kept) in meta.clocks {
            let held = Clock::load(id, clock, kept, group.keys.clone(), step).await?;
            group.clocks.insert(clock, held);
        }

        Ok(Some(group))
    }

    /// How many entries were dropped and are still stored.
    #[cfg(test)]
    pub(crate) fn burying(&self) -> usize {
        let clocks = self.clocks.values();

        clocks.map(|clock| clock.condemned.len()).sum()
    }

    /// The height of the last checkpoint committed here.
    #[cfg(test)]
    pub(crate) fn folded(&self) -> u64 {
        self.folded
    }

    /// The height of the last checkpoint any of which is committed
    /// here.
    #[cfg(test)]
    pub(crate) fn reached(&self) -> u64 {
        let installing = self.installing.as_ref().map(|it| it.height);

        self.folding.or(installing).unwrap_or(self.folded)
    }

    /// How many verdicts on entries have been worked out one at a
    /// time, rather than all together.
    #[cfg(test)]
    pub(crate) fn weighed(&self) -> usize {
        self.clocks
            .values()
            .map(|clock| clock.verdicts.weighed)
            .sum()
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

    /// The rank of the highest attempt it knows of at the next
    /// checkpoint.
    #[cfg(test)]
    pub(crate) fn attempt(&self) -> Option<u64> {
        self.chain.attempt()
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

    fn persist<V: View>(&mut self, step: &mut Step<V>) {
        let clocks = self.clocks.iter();
        let meta = Meta {
            clocks: clocks.map(|(&id, clock)| (id, clock.kept())).collect(),
            folded: self.folded,
            base: self.base,
            staging: self.staging.clone(),
            wanted: self.wanted.clone(),
            requests: self.requests.values().cloned().collect(),
            origin: self.origin,
            interest: self.interest.clone(),
            passed: self.passed,
            folding: self.folding,
            littered: self.fore.littered,
            installing: self.installing.clone(),
            mending: self.mending,
            keys: self.keys.clone(),
            carrying: self.carrying,
            renewing: self.renewing,
            skipping: self.skipping,
            spoke: crate::model::vocabulary(),
            extended: self.extended.clone(),
        };
        step.save(keys::meta(self.id), &meta);
        if self.said {
            step.save(keys::chain(self.id), &self.chain.save());
            self.store_links(step);
        }
    }

    /// Brings the decided checkpoints in storage in line with those
    /// held: the new ones in, and those shed out.
    fn store_links<V: View>(&mut self, step: &mut Step<V>) {
        let held = self.chain.links().iter();
        let held: BTreeSet<u64> = held.map(|link| link.checkpoint.height).collect();
        for link in self.chain.links() {
            if !self.stored.contains(&link.checkpoint.height) {
                step.save(keys::link(self.id, link.checkpoint.height), link);
            }
        }
        for &height in self.stored.difference(&held) {
            step.delete(keys::link(self.id, height));
        }
        self.stored = held;
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
        #[cfg(test)]
        let tip = Checkpoint {
            body: [&tip.body[..], &self.misnaming].concat(),
            ..tip
        };
        // A device that learns it forked has lost track of what it
        // signed, and anything more it wrote would be dropped unread.
        let me = identity.peer();
        let may = tip.electorate.writers(clock).includes(me) && self.wants(clock);
        if !may || self.proofs.contains_key(&me) {
            return Ok(Wrote::NotAllowed);
        }
        let Some(seq) = self.clock(clock).write(identity, &tip, operations, step) else {
            return Ok(Wrote::Unreadable);
        };

        self.progress(identity, None, step).await?;

        Ok(Wrote::Yes(seq))
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

        self.request(identity, electorate.clone());
        self.wanted = Some(electorate);
        self.said = true;
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
        self.compact(only);

        self.progress(identity, None, step).await
    }

    /// Time has passed. It's when a checkpoint is attempted, and when a
    /// stalled attempt is retried.
    /// Whether there's more to do that waits for nothing: see
    /// [`resume`].
    ///
    /// [`resume`]: Self::resume
    pub(crate) fn is_busy(&self) -> bool {
        let unswept = self.clocks.values().any(Clock::is_unswept);

        self.is_partway()
            || self.is_ahead()
            || unswept
            || self.is_indexing()
            || self.is_inheriting()
    }

    /// Which checkpoint a committed read is of right now.
    pub(crate) fn committed_at(&self) -> Reached {
        Reached {
            checkpoint: self.folded,
            settled: !self.is_partway(),
        }
    }

    /// Whether committed state is partway between two checkpoints, and
    /// so the state of neither.
    fn is_partway(&self) -> bool {
        self.folding.is_some() || self.installing.is_some()
    }

    /// Carries on with what an earlier step left unfinished.
    pub(crate) async fn resume<V: View>(
        &mut self,
        identity: &Identity,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        self.progress(identity, None, step).await
    }

    pub(crate) async fn tick<V: View>(
        &mut self,
        identity: &Identity,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        // An attempt that's getting somewhere is left to it.
        let gathered = self.chain.gathered();
        let moving = std::mem::replace(&mut self.gathered, gathered) != gathered;
        if self.chain.attempt().is_some() && !moving {
            self.stalled += 1;
            self.said = true;
        } else {
            self.stalled = 0;
        }

        for remote in self.remotes.values_mut() {
            remote.doubt();
        }

        self.fore.due = true;
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
        self.install(step).await?;
        self.sweep(step).await?;
        if std::mem::take(&mut self.fore.due) || self.fore.proposing.is_some() {
            self.propose(identity, step).await?;
        }
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
        self.look_back(step).await?;
        self.bury(step);
        let caught_up = self.folded == self.chain.tip().height;
        if self
            .skipping
            .is_some_and(|from| self.folded > from && caught_up)
        {
            self.skipping = None;
        }

        self.clocks
            .retain(|_, clock| !clock.is_empty() || clock.is_unswept());
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
