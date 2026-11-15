//! One group, as one device holds it: its checkpoint chain, everyone's
//! entries, and which of them have happened.

mod commit;
mod history;
mod judge;
mod sync;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::actor::{Identity, Peer};
use crate::agreement::AgreementId;
use crate::chain::{Chain, Checkpoint, Electorate, Saved};
use crate::clock::Stamp;
use crate::encoding;
use crate::model::{ModelId, Operation};
use crate::storage::keys;
use crate::store::{StoreError, View};

use super::entry::{Checked, Entry, Frontier, Hash, Header, Outline, reach};
use super::step::Step;
use history::History;
use judge::Written;
use sync::Remote;

#[derive(Debug)]
pub(crate) struct Group {
    id: AgreementId,
    chain: Chain,
    history: History,
    /// The height of the last checkpoint whose entries are committed
    /// here. Behind the chain's tip while those entries are on their way.
    folded: u64,
    /// Models dropped by committed entries.
    dropped: BTreeSet<ModelId>,
    /// The uncommitted entries that happen, as storage shows them.
    live: BTreeSet<Stamp>,
    /// What each connected peer is thought to hold.
    remotes: BTreeMap<Peer, Remote>,
    /// The electorate this device has been asked to bring about.
    wanted: Option<Electorate>,
    /// How many ticks the current attempt at a checkpoint has gone on.
    stalled: u64,
    /// Whether the chain has anything peers haven't been told.
    said: bool,
    /// Whether verdicts need working out again.
    stale: bool,
}

/// What's kept of a group besides its chain and its entries.
#[derive(Debug, Serialize, Deserialize)]
struct Meta {
    heads: Frontier,
    committed: Frontier,
    folded: u64,
    dropped: BTreeSet<ModelId>,
    wanted: Option<Electorate>,
}

/// A group is named by the checkpoint it starts from.
pub(crate) fn id_of(founding: &Checkpoint) -> AgreementId {
    AgreementId::from_bytes(*founding.digest().as_bytes())
}

impl Group {
    fn new(id: AgreementId, chain: Chain) -> Self {
        Self {
            id,
            chain,
            history: History::default(),
            folded: 0,
            dropped: BTreeSet::new(),
            live: BTreeSet::new(),
            remotes: BTreeMap::new(),
            wanted: None,
            stalled: 0,
            said: false,
            stale: false,
        }
    }

    /// Starts holding a group, from its first checkpoint.
    pub(crate) fn start<V: View>(founding: Checkpoint, step: &mut Step<V>) -> Self {
        let id = id_of(&founding);
        step.save(keys::genesis(id), &founding);
        let mut group = Self::new(id, Chain::new(founding));
        group.said = true;
        group.persist(step);

        group
    }

    /// Picks a group up from storage.
    pub(crate) async fn load<V: View>(
        id: AgreementId,
        step: &Step<V>,
    ) -> Result<Option<Self>, StoreError> {
        let saved = step.load::<Saved>(&keys::view(id)).await?;
        let meta = step.load::<Meta>(&keys::clock(id)).await?;
        let (Some(saved), Some(meta)) = (saved, meta) else {
            return Ok(None);
        };

        let mut group = Self::new(id, Chain::restore(saved));
        group.folded = meta.folded;
        group.dropped = meta.dropped;
        group.wanted = meta.wanted;
        group.history = History::resume(meta.heads, meta.committed);
        for (author, head) in group.history.heads.clone() {
            let from = reach(&group.history.committed, author) + 1;
            let (start, end) = (
                keys::log(id, author, from),
                keys::log(id, author, head.seq + 1),
            );
            for (_, stored) in step.scan(&start, &end).await? {
                let entry = encoding::decode::<Entry>(&stored).and_then(Entry::trusted);
                let entry = entry.ok_or_else(|| StoreError::new("a stored entry is unreadable"))?;
                group.history.remember(entry.outline());
            }
        }

        // Storage already shows these: verdicts depend on nothing but
        // what was stored along with them.
        group.live = group.judged(step).await?;

        Ok(Some(group))
    }

    /// The height of the last checkpoint committed here.
    #[cfg(test)]
    pub(crate) fn folded(&self) -> u64 {
        self.folded
    }

    pub(crate) fn founding(&self) -> Option<&Checkpoint> {
        self.chain.checkpoints().next()
    }

    fn persist<V: View>(&self, step: &mut Step<V>) {
        let meta = Meta {
            heads: self.history.heads.clone(),
            committed: self.history.committed.clone(),
            folded: self.folded,
            dropped: self.dropped.clone(),
            wanted: self.wanted.clone(),
        };
        step.save(keys::clock(self.id), &meta);
        if self.said {
            step.save(keys::view(self.id), &self.chain.save());
        }
    }

    /// Appends an entry to this device's own chain. Returns whether it
    /// may: only a contributor writes.
    pub(crate) async fn write<V: View>(
        &mut self,
        identity: &Identity,
        mut operations: Vec<Operation>,
        step: &mut Step<V>,
    ) -> Result<bool, StoreError> {
        let me = identity.peer();
        let tip = self.chain.tip();
        if !tip.electorate.includes(me) {
            return Ok(false);
        }

        let seq = reach(&self.history.heads, me) + 1;
        for (index, operation) in operations.iter_mut().enumerate() {
            operation.seal(me, seq, index);
        }

        let mut after = self.history.heads.clone();
        after.remove(&me);
        let header = Header {
            group: self.id,
            seq,
            previous: self.history.hash_of_head(me),
            after,
            lamport: self.history.lamport() + 1,
            checkpoint: tip.digest(),
            payload: Hash::NONE,
        };
        let entry = Entry::author(identity, header, operations);
        self.integrate(&entry, step);
        self.progress(identity, None, step).await?;

        Ok(true)
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
        if !allowed || electorate.is_empty() {
            return Ok(false);
        }

        self.wanted = Some(electorate);
        self.progress(identity, None, step).await?;

        Ok(true)
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

        self.propose(identity);
        self.progress(identity, None, step).await
    }

    fn integrate<V: View>(&mut self, entry: &Checked, step: &mut Step<V>) {
        let key = keys::log(self.id, entry.author(), entry.header().seq);
        step.save(key, &entry.entry);
        self.history.add(entry.outline());
        self.stale = true;
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
            let moved = self.advance(identity);
            self.said |= moved;
            let folded = self.fold(step).await?;
            if !moved && !folded {
                break;
            }
        }

        if self.stale {
            self.rejudge(step).await?;
            self.stale = false;
        }

        self.persist(step);
        let peers: Vec<Peer> = self.remotes.keys().copied().collect();
        for peer in peers {
            self.push(peer, owed == Some(peer), self.said, step).await?;
        }
        self.said = false;

        Ok(())
    }

    /// Which uncommitted entries happen.
    async fn judged<V: View>(&self, step: &Step<V>) -> Result<BTreeSet<Stamp>, StoreError> {
        let mut written = Written::on(&self.dropped);
        let mut live = BTreeSet::new();
        for (&stamp, entry) in &self.history.uncommitted {
            if judge::weigh(self.id, entry, &written, step).await? {
                written.apply(&entry.declared);
                live.insert(stamp);
            }
        }

        Ok(live)
    }

    /// Works out every uncommitted verdict again, and makes storage show
    /// exactly the entries that happen.
    async fn rejudge<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        let live = self.judged(step).await?;
        for stamp in live.symmetric_difference(&self.live) {
            let Some(entry) = self.history.uncommitted.get(stamp) else {
                continue;
            };

            if live.contains(stamp) {
                let operations = self.operations(entry, step).await?;
                judge::show(self.id, *stamp, &operations, step);
            } else {
                judge::hide(self.id, entry, step);
            }
            judge::notify(self.id, entry, false, step);
        }
        self.live = live;

        Ok(())
    }

    /// Reads back what an entry does. Only its outline is kept in memory.
    async fn operations<V: View>(
        &self,
        entry: &Outline,
        step: &Step<V>,
    ) -> Result<Vec<Operation>, StoreError> {
        let key = keys::log(self.id, entry.author, entry.link.seq);
        let stored = step.load::<Entry>(&key).await?;
        let stored = stored.and_then(Entry::trusted);

        stored
            .map(|entry| entry.operations)
            .ok_or_else(|| StoreError::new("an entry is missing from the store"))
    }
}
