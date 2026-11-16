//! One clock, as one device holds it: everyone's entries in it, which
//! of them have happened, and the state they've committed.
//!
//! Clocks in a group share a checkpoint chain and nothing else. An
//! entry in one is never ordered against, judged against, or made to
//! wait for an entry in another.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::actor::{Identity, Peer};
use crate::chain::{Checkpoint, Writers};
use crate::clock::Stamp;
use crate::db::{Changed, ClockId, GroupId, Pending, Waiting, keys};
use crate::encoding;
use crate::model::{ModelId, Operation};
use crate::seal::Keyring;
use crate::store::{StoreError, View};

use super::super::entry::{Checked, Entry, Frontier, Hash, Header, Outline, Under, reach};
use super::super::step::Step;
use super::deferred;
use super::history::History;
use super::judge::{self, Written};
use super::root::{Accumulator, Root};
use super::rules;

#[derive(Debug)]
pub(super) struct Clock {
    pub(super) group: GroupId,
    pub(super) id: ClockId,
    pub(super) history: History,
    /// The last entry of each author's chain that's been shed.
    pub(super) shed: Frontier,
    /// Whether this device is getting back what it shed.
    pub(super) restoring: bool,
    /// The height of the checkpoint whose entries were last shed. A
    /// peer that hasn't committed that far can't be sent what it lacks.
    pub(super) base: u64,
    /// Models dropped by committed entries.
    pub(super) dropped: BTreeSet<ModelId>,
    /// What this device can read of the group.
    pub(super) keys: Keyring,
    /// Whether an entry was committed that this device couldn't read.
    /// Its state is behind from then on, and only a snapshot mends it.
    lost: bool,
    /// The uncommitted entries that happen, as storage shows them.
    live: BTreeSet<Stamp>,
    /// Whether verdicts need working out again.
    pub(super) stale: bool,
}

/// What's kept of a clock besides its entries and its state.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Kept {
    heads: Frontier,
    committed: Frontier,
    shed: Frontier,
    restoring: bool,
    base: u64,
    dropped: BTreeSet<ModelId>,
    lost: bool,
}

/// What committing a checkpoint's entries comes to, in one clock.
pub(super) struct Settled {
    pub(super) state: Root,
    dropped: BTreeSet<ModelId>,
    /// Whether any of it went unread, and so undone.
    pub(super) lost: bool,
    /// The covered entries, in timeline order, and whether each happens.
    verdicts: Vec<(Stamp, bool)>,
    /// The keys written by changes that were worked out on the way.
    /// No entry declared them.
    resolved: Vec<(ModelId, Vec<u8>)>,
}

impl Clock {
    pub(super) fn new(group: GroupId, id: ClockId) -> Self {
        Self {
            group,
            id,
            history: History::default(),
            shed: Frontier::new(),
            restoring: false,
            base: 0,
            dropped: BTreeSet::new(),
            keys: Keyring::Open,
            lost: false,
            live: BTreeSet::new(),
            stale: false,
        }
    }

    /// Picks a clock up from storage.
    pub(super) async fn load<V: View>(
        group: GroupId,
        id: ClockId,
        kept: Kept,
        keys: Keyring,
        step: &Step<V>,
    ) -> Result<Self, StoreError> {
        let mut clock = Self::new(group, id);
        clock.keys = keys;
        clock.lost = kept.lost;
        clock.shed = kept.shed;
        clock.base = kept.base;
        clock.restoring = kept.restoring;
        clock.dropped = kept.dropped;
        clock.history = History::resume(kept.heads, kept.committed);
        for (author, head) in clock.history.heads.clone() {
            let from = reach(&clock.history.committed, author) + 1;
            let (start, end) = (
                keys::log(group, id, author, from),
                keys::log(group, id, author, head.seq + 1),
            );
            for (_, stored) in step.scan(&start, &end).await? {
                let entry = encoding::decode::<Entry>(&stored);
                let entry = entry.ok_or_else(|| StoreError::new("a stored entry is unreadable"))?;
                clock.history.remember(entry.trusted(&clock.keys).outline());
            }
        }

        // Storage already shows these: verdicts depend on nothing but
        // what was stored along with them.
        clock.live = clock.judged(step).await?;

        Ok(clock)
    }

    pub(super) fn kept(&self) -> Kept {
        Kept {
            heads: self.history.heads.clone(),
            committed: self.history.committed.clone(),
            shed: self.shed.clone(),
            base: self.base,
            restoring: self.restoring,
            dropped: self.dropped.clone(),
            lost: self.lost,
        }
    }

    /// Whether this device can't say what the clock holds: there's an
    /// entry in it that it can't read. Nothing shows of what isn't
    /// committed, nothing more is applied, and nothing is vouched for.
    /// What's read is what was committed before.
    pub(super) fn is_blind(&self) -> bool {
        self.lost || self.history.uncommitted.values().any(|entry| entry.unread)
    }

    /// Whether nothing was ever written here, as far as this device
    /// knows. Such a clock isn't one yet.
    pub(super) fn is_empty(&self) -> bool {
        self.history.heads.is_empty() && self.history.committed.is_empty()
    }

    /// Appends an entry to this device's own chain. Returns whether it
    /// could: a device that can't read the clock doesn't write to it.
    pub(super) fn write<V: View>(
        &mut self,
        identity: &Identity,
        tip: &Checkpoint,
        mut operations: Vec<Operation>,
        step: &mut Step<V>,
    ) -> bool {
        if self.is_blind() {
            return false;
        }

        let me = identity.peer();
        let seq = reach(&self.history.heads, me) + 1;
        for (index, operation) in operations.iter_mut().enumerate() {
            operation.seal(me, seq, index);
        }

        let mut after = self.history.heads.clone();
        after.remove(&me);
        let header = Header {
            group: self.group,
            clock: self.id,
            seq,
            previous: self.history.hash_of_head(me),
            after,
            lamport: self.history.lamport() + 1,
            under: Under {
                height: tip.height,
                checkpoint: tip.digest(),
            },
            payload: Hash::NONE,
        };
        let Some(entry) = Entry::author(identity, header, operations, &self.keys) else {
            return false;
        };
        self.integrate(&entry, step);

        true
    }

    pub(super) fn integrate<V: View>(&mut self, entry: &Checked, step: &mut Step<V>) {
        let key = keys::log(self.group, self.id, entry.author(), entry.header().seq);
        step.save(key, &entry.entry);
        self.history.add(entry.outline());
        self.stale = true;
    }

    /// The changes waiting to be committed, of the entries that
    /// happen, in timeline order.
    pub(super) async fn pending<V: View>(
        &self,
        step: &Step<V>,
    ) -> Result<Vec<Pending>, StoreError> {
        let mut pending = Vec::new();
        for stamp in &self.live {
            let entry = self.history.uncommitted.get(stamp);
            let Some(entry) = entry.filter(|entry| !entry.waits.is_empty()) else {
                continue;
            };

            for operation in self.operations(entry, step).await? {
                let change = match operation.waits() {
                    Some(deferred) => deferred.into(),
                    None if operation.drops() => Waiting::Drop,
                    None => continue,
                };
                pending.push(Pending {
                    model: operation.model,
                    author: entry.author.id(),
                    change,
                });
            }
        }

        Ok(pending)
    }

    /// Which uncommitted entries happen.
    async fn judged<V: View>(&self, step: &Step<V>) -> Result<BTreeSet<Stamp>, StoreError> {
        let mut written = Written::on(&self.dropped);
        let mut live = BTreeSet::new();
        if self.is_blind() {
            return Ok(live);
        }

        for (&stamp, entry) in &self.history.uncommitted {
            if judge::weigh(self.group, entry, &written, step).await? {
                written.apply(&entry.declared);
                live.insert(stamp);
            }
        }

        Ok(live)
    }

    /// Works out every uncommitted verdict again, if anything could
    /// have changed one, and makes storage show exactly the entries
    /// that happen.
    pub(super) async fn rejudge<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        if !self.stale {
            return Ok(());
        }

        let live = self.judged(step).await?;
        for stamp in live.symmetric_difference(&self.live) {
            let Some(entry) = self.history.uncommitted.get(stamp) else {
                continue;
            };

            if live.contains(stamp) {
                let operations = self.operations(entry, step).await?;
                judge::show(self.group, *stamp, &operations, step);
            } else {
                judge::hide(self.group, entry, step);
            }
            judge::notify(self.group, entry, false, step);
        }
        self.live = live;
        self.stale = false;

        Ok(())
    }

    /// Reads back what an entry does. Only its outline is kept in memory.
    async fn operations<V: View>(
        &self,
        entry: &Outline,
        step: &Step<V>,
    ) -> Result<Vec<Operation>, StoreError> {
        let key = keys::log(self.group, self.id, entry.author, entry.link.seq);
        let stored = step.load::<Entry>(&key).await?;

        stored
            .map(|entry| entry.trusted(&self.keys).operations)
            .ok_or_else(|| StoreError::new("an entry is missing from the store"))
    }

    /// Commits, in `step`, every uncommitted entry `frontier` covers:
    /// each is judged against committed state alone, which by then
    /// includes every entry before it. Nothing in memory changes, so a
    /// step that's thrown away leaves no trace.
    pub(super) async fn settle<V: View>(
        &self,
        frontier: &Frontier,
        step: &mut Step<V>,
    ) -> Result<Settled, StoreError> {
        let stored = step.get(&keys::state(self.group, self.id)).await?;
        let stored = stored.and_then(|bytes| Accumulator::from_bytes(&bytes));
        let mut state = stored.unwrap_or_default();
        let mut dropped = self.dropped.clone();
        let mut verdicts = Vec::new();
        let mut resolved = Vec::new();

        // Committing what can't be read would be guessing. The state
        // stays where it was, and is behind from here on.
        if self.is_blind() {
            let covered = self.history.covered(frontier);
            verdicts.extend(covered.map(|entry| (entry.stamp(), false)));

            return Ok(Settled {
                state: state.root(&self.keys),
                dropped,
                lost: !verdicts.is_empty(),
                verdicts,
                resolved,
            });
        }

        for entry in self.history.covered(frontier) {
            let mut happens = judge::weigh(self.group, entry, &Written::on(&dropped), step).await?;
            if happens {
                let operations = self.operations(entry, step).await?;
                let waits = operations.iter().any(|it| it.waits().is_some());
                if waits || rules::bind(self.group, &operations, step).await? {
                    // Whether it can all be done isn't known until
                    // it's been tried, and it's all or nothing.
                    let (mut trial, mut sum) = (step.fork(), state.clone());
                    let tried = deferred::carry_out(
                        self.group,
                        &operations,
                        &dropped,
                        &mut sum,
                        &mut trial,
                    );
                    happens = tried.await?.is_some();
                    if happens {
                        let done = deferred::carry_out(
                            self.group,
                            &operations,
                            &dropped,
                            &mut state,
                            step,
                        );
                        resolved.extend(done.await?.unwrap_or_default());
                    }
                } else {
                    judge::commit(self.group, &operations, &mut state, step).await?;
                }

                let drops = operations.iter().filter(|it| happens && it.drops());
                dropped.extend(drops.map(|it: &Operation| it.model));
            }
            verdicts.push((entry.stamp(), happens));
        }

        step.put(keys::state(self.group, self.id), state.to_bytes());

        Ok(Settled {
            state: state.root(&self.keys),
            dropped,
            lost: false,
            verdicts,
            resolved,
        })
    }

    /// Takes what [`settle`] did in the same step as done: the entries
    /// `frontier` covers are committed, and no longer anything else.
    ///
    /// [`settle`]: Self::settle
    pub(super) fn commit<V: View>(
        &mut self,
        frontier: &Frontier,
        settled: Settled,
        step: &mut Step<V>,
    ) {
        self.dropped = settled.dropped;
        self.lost |= settled.lost;
        for (stamp, happened) in settled.verdicts {
            let Some(entry) = self.history.forget(stamp) else {
                continue;
            };

            if happened {
                judge::notify(self.group, &entry, true, step);
            }
            judge::hide(self.group, &entry, step);
            judge::notify(self.group, &entry, false, step);
            self.live.remove(&stamp);
        }

        for (model, key) in settled.resolved {
            step.changed(Changed::Key {
                group: self.group,
                model,
                key,
                committed: true,
            });
        }

        self.history.commit(frontier);
        self.stale = true;
    }

    /// Lets go of whatever was written, past the last checkpoint, by
    /// anyone `writers` leaves out. That's the whole of removal: a
    /// removed contributor's history ends at the checkpoint that
    /// removed it.
    pub(super) fn cut<V: View>(&mut self, writers: Writers<'_>, height: u64, step: &mut Step<V>) {
        // An entry written under a later checkpoint was written by
        // someone let back in since. This device is only now catching
        // up to where they were stopped.
        let earlier = self.history.uncommitted.values();
        let earlier = earlier.filter(|entry| entry.under <= height);
        let authors = earlier.map(|entry| entry.author);
        let removed = authors.filter(|&author| !writers.includes(author));
        let removed: BTreeSet<Peer> = removed.collect();
        for author in removed {
            self.void(author, 0, step);
        }
    }

    /// Drops everything `author` wrote past `seq` that isn't committed.
    pub(super) fn void<V: View>(&mut self, author: Peer, seq: u64, step: &mut Step<V>) {
        let entries = self.history.cut(author, seq);
        if entries.is_empty() {
            return;
        }

        tracing::info!(
            group = ?self.group,
            clock = ?self.id,
            ?author,
            entries = entries.len(),
            "dropped entries that will never count"
        );
        for entry in entries {
            step.delete(keys::log(self.group, self.id, author, entry.link.seq));
            judge::hide(self.group, &entry, step);
            judge::notify(self.group, &entry, false, step);
            self.live.remove(&entry.stamp());
        }
        self.stale = true;
    }

    /// Sheds every entry `frontier` covers.
    pub(super) async fn shed_through<V: View>(
        &mut self,
        frontier: &Frontier,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        for (&author, link) in frontier {
            let from = reach(&self.shed, author) + 1;
            let start = keys::log(self.group, self.id, author, from);
            let end = keys::log(self.group, self.id, author, link.seq + 1);
            for (key, _) in step.scan(&start, &end).await? {
                step.delete(key);
            }
            if from <= link.seq {
                self.shed.insert(author, *link);
            }
        }

        Ok(())
    }

    /// Takes on committed state that came from a snapshot. Whatever is
    /// held that `frontier` covers is already in it.
    pub(super) async fn adopt<V: View>(
        &mut self,
        frontier: &Frontier,
        dropped: BTreeSet<ModelId>,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        // Whatever is held in place of an entry the checkpoint names
        // was never going to count, and neither was anything written
        // on top of it.
        for author in self.history.replaced(frontier) {
            self.void(author, 0, step);
        }

        self.dropped = dropped;
        let covered = self.history.covered(frontier);
        let covered: Vec<_> = covered.map(Outline::stamp).collect();
        for stamp in covered {
            if let Some(entry) = self.history.forget(stamp) {
                judge::hide(self.group, &entry, step);
                self.live.remove(&stamp);
            }
        }

        self.shed_through(frontier, step).await?;
        self.history.adopt(frontier);
        self.lost = false;
        self.stale = true;

        Ok(())
    }

    /// The first `page` entries of each chain that a peer lacks, if it
    /// `said` it holds this much and has been `sent` this much since.
    /// What it was sent isn't sent again, whatever it says it holds: it
    /// may be holding another entry in that place, and keeping it.
    pub(super) async fn missing<V: View>(
        &self,
        said: &Frontier,
        sent: &Frontier,
        page: u64,
        step: &Step<V>,
    ) -> Result<Vec<(Stamp, Entry)>, StoreError> {
        let mut missing = Vec::new();
        for (&author, head) in &self.history.heads {
            let said = self.history.shared(author, said.get(&author));
            let sent = self.history.shared(author, sent.get(&author));
            let held = said.max(sent);
            if held >= head.seq {
                continue;
            }

            let last = head.seq.min(held.saturating_add(page));
            let start = keys::log(self.group, self.id, author, held + 1);
            let end = keys::log(self.group, self.id, author, last + 1);
            for (_, stored) in step.scan(&start, &end).await? {
                let Some(entry) = encoding::decode::<Entry>(&stored) else {
                    return Err(StoreError::new("a stored entry is unreadable"));
                };

                let lamport = entry.header.payload().lamport;
                missing.push((Stamp { lamport, author }, entry));
            }
        }

        Ok(missing)
    }
}
