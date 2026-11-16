//! One clock, as one device holds it: everyone's entries in it, which
//! of them have happened, and the state they've committed.
//!
//! Clocks in a group share a checkpoint chain and nothing else. An
//! entry in one is never ordered against, judged against, or made to
//! wait for an entry in another.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::actor::{Identity, Peer};
use crate::chain::{Checkpoint, Writers};
use crate::clock::Stamp;
use crate::db::{ClockId, GroupId, Pending, Waiting, keys};
use crate::encoding;
use crate::model::{ModelId, Operation};
use crate::seal::Keyring;
use crate::store::{StoreError, View};

use super::super::entry::{Checked, Entry, Frontier, Hash, Header, Link, Outline, Under, reach};
use super::super::step::Step;
use super::history::History;
use super::job::Job;
use super::judge::{self, Written};
use super::root::{Accumulator, Root};
use super::rules;

#[derive(Clone, Debug)]
pub(super) struct Clock {
    pub(super) group: GroupId,
    pub(super) id: ClockId,
    pub(super) history: History,
    /// The last entry of each author's chain that's been shed.
    pub(super) shed: Frontier,
    /// How far along each author's chain what's shed has been deleted.
    /// Shedding is a decision, made at once. Deleting follows, so much
    /// a step.
    pub(super) swept: BTreeMap<Peer, u64>,
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
    /// The transaction being committed, if that takes more than a
    /// step.
    pub(super) job: Option<Job>,
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
    swept: BTreeMap<Peer, u64>,
    restoring: bool,
    base: u64,
    dropped: BTreeSet<ModelId>,
    lost: bool,
    job: Option<Job>,
}

/// What committing a checkpoint's entries comes to, in one clock.
pub(super) struct Settled {
    pub(super) dropped: BTreeSet<ModelId>,
    /// Whether any of it went unread, and so undone.
    pub(super) lost: bool,
    /// The entries settled, in timeline order, and whether each
    /// happens.
    pub(super) verdicts: Vec<(Stamp, bool)>,
    /// How far along the timeline that got.
    pub(super) through: Frontier,
    /// The transaction that comes next, if committing it is a
    /// [job](Job).
    pub(super) job: Option<Job>,
}

impl Clock {
    pub(super) fn new(group: GroupId, id: ClockId) -> Self {
        Self {
            group,
            id,
            history: History::default(),
            shed: Frontier::new(),
            swept: BTreeMap::new(),
            restoring: false,
            base: 0,
            dropped: BTreeSet::new(),
            keys: Keyring::Open,
            lost: false,
            job: None,
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
        clock.swept = kept.swept;
        clock.job = kept.job;
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
            swept: self.swept.clone(),
            base: self.base,
            restoring: self.restoring,
            dropped: self.dropped.clone(),
            lost: self.lost,
            job: self.job.clone(),
        }
    }

    /// Whether this device can't say what the clock holds: there's an
    /// entry in it that it can't read. Nothing shows of what isn't
    /// committed, nothing more is applied, and nothing is vouched for.
    /// What's read is what was committed before.
    pub(super) fn is_blind(&self) -> bool {
        self.lost || self.history.uncommitted.values().any(|entry| entry.unread)
    }

    /// Whether something committed here went unread, so that what
    /// this device has for the clock's state isn't its state.
    pub(super) fn is_lost(&self) -> bool {
        self.lost
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
    pub(super) async fn operations<V: View>(
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
        let dropped = self.dropped.clone();
        let mut verdicts = Vec::new();

        // Committing what can't be read would be guessing. The state
        // stays where it was, and is behind from here on.
        if self.is_blind() {
            let covered = self.history.covered(frontier);
            verdicts.extend(covered.map(|entry| (entry.stamp(), false)));

            return Ok(Settled {
                dropped,
                lost: !verdicts.is_empty(),
                verdicts,
                through: frontier.clone(),
                job: None,
            });
        }

        let mut through = self.history.committed.clone();
        let mut job = None;
        for entry in self.history.covered(frontier) {
            let happens = judge::weigh(self.group, entry, &Written::on(&dropped), step).await?;
            if happens {
                let operations = self.operations(entry, step).await?;
                let waits = operations.iter().any(|it| it.waits().is_some());
                let drops = operations.iter().filter(|it| it.drops());
                let drops: BTreeSet<ModelId> = drops.map(|it: &Operation| it.model).collect();

                // Whether it can all be done isn't known until it's
                // been tried, and it's all or nothing. That, or taking
                // a model's values away, is more than this does.
                if waits || rules::bind(self.group, &operations, step).await? {
                    job = Some(Job::trying(entry.stamp()));
                    break;
                }

                judge::commit(self.group, &operations, &mut state, step).await?;
                if !drops.is_empty() {
                    job = Some(Job::emptying(entry.stamp(), drops));
                    break;
                }
            }
            verdicts.push((entry.stamp(), happens));
            through.insert(entry.author, entry.link);
        }

        step.put(keys::state(self.group, self.id), state.to_bytes());

        Ok(Settled {
            dropped,
            lost: false,
            verdicts,
            through,
            job,
        })
    }

    /// The name of the clock's committed state, as `step` has it.
    pub(super) async fn root<V: View>(&self, step: &Step<V>) -> Result<Root, StoreError> {
        let stored = step.get(&keys::state(self.group, self.id)).await?;
        let stored = stored.and_then(|bytes| Accumulator::from_bytes(&bytes));

        Ok(stored.unwrap_or_default().root(&self.keys))
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

    /// Sheds every entry `frontier` covers. They're as good as gone
    /// from here on, and are deleted by [`sweep`].
    ///
    /// [`sweep`]: Self::sweep
    pub(super) fn shed_through(&mut self, frontier: &Frontier) {
        for (&author, link) in frontier {
            if reach(&self.shed, author) < link.seq {
                self.shed.insert(author, *link);
            }
        }
    }

    /// Whether anything shed is still stored.
    pub(super) fn is_unswept(&self) -> bool {
        let swept = |(author, link): (&Peer, &Link)| {
            self.swept.get(author).copied().unwrap_or(0) >= link.seq
        };

        !self.shed.iter().all(swept)
    }

    /// Deletes up to `budget` entries that were shed. Returns how many.
    pub(super) async fn sweep<V: View>(
        &mut self,
        mut budget: usize,
        step: &mut Step<V>,
    ) -> Result<usize, StoreError> {
        let mut deleted = 0;
        for (&author, link) in &self.shed {
            let swept = self.swept.get(&author).copied().unwrap_or(0);
            if swept >= link.seq || budget == 0 {
                continue;
            }

            let start = keys::log(self.group, self.id, author, swept + 1);
            let end = keys::log(self.group, self.id, author, link.seq + 1);
            let page = step.scan_page(&start, &end, budget).await?;
            let last = page.last().filter(|_| page.len() == budget);
            let last = last.and_then(|(key, _)| key.last_chunk::<8>());
            let reached = last.map_or(link.seq, |seq| u64::from_be_bytes(*seq));
            budget -= page.len();
            deleted += page.len();
            for (key, _) in page {
                step.delete(key);
            }
            self.swept.insert(author, reached);
        }

        Ok(deleted)
    }

    /// Takes on committed state that came from a snapshot. Whatever is
    /// held that `frontier` covers is already in it.
    pub(super) fn adopt<V: View>(
        &mut self,
        frontier: &Frontier,
        dropped: BTreeSet<ModelId>,
        step: &mut Step<V>,
    ) {
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

        self.shed_through(frontier);
        self.history.adopt(frontier);
        self.lost = false;
        self.stale = true;
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
