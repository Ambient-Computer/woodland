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
use crate::db::{ClockId, GroupId, Outcome, Pending, Waiting, keys};
use crate::encoding;
use crate::model::{ModelId, Operation};
use crate::seal::Keyring;
use crate::store::{StoreError, View};

use super::super::entry::{Checked, Entry, Frontier, Hash, Header, Link, Outline, Under, reach};
use super::super::step::{Step, Told};
use super::history::History;
use super::job::Job;
use super::judge::{self, Written};
use super::root::{Accumulator, Root};
use super::rules;
use super::verdicts::Verdicts;

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
    /// Which uncommitted entries happen.
    pub(super) verdicts: Verdicts,
    /// Entries that will never count and are still stored. Dropping
    /// them is a decision, made at once. Deleting follows, so much a
    /// step. They're past the end of their author's chain, always.
    pub(super) condemned: BTreeMap<(Peer, u64), Outline>,
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
    /// How far past the end of each author's chain there are entries
    /// stored that never counted.
    condemned: BTreeMap<Peer, u64>,
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
            verdicts: Verdicts::default(),
            condemned: BTreeMap::new(),
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

        for (author, last) in kept.condemned {
            let first = reach(&clock.history.heads, author) + 1;
            let (start, end) = (
                keys::log(group, id, author, first),
                keys::log(group, id, author, last.saturating_add(1)),
            );
            for (_, stored) in step.scan(&start, &end).await? {
                let entry = encoding::decode::<Entry>(&stored);
                let entry = entry.ok_or_else(|| StoreError::new("a stored entry is unreadable"))?;
                let outline = entry.trusted(&clock.keys).outline();
                clock.condemned.insert((author, outline.link.seq), outline);
            }
        }

        // Storage already shows these: verdicts depend on nothing but
        // what was stored along with them.
        clock.recall(step).await?;

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
            condemned: self.last_condemned(),
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
    ) -> Option<u64> {
        if self.is_blind() {
            return None;
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
        let entry = Entry::author(identity, header, operations, &self.keys)?;
        self.integrate(&entry, step);

        Some(seq)
    }

    pub(super) fn integrate<V: View>(&mut self, entry: &Checked, step: &mut Step<V>) {
        self.make_way(entry.author(), entry.header().seq, step);
        let key = keys::log(self.group, self.id, entry.author(), entry.header().seq);
        step.save(key, &entry.entry);
        let outline = entry.outline();
        self.verdicts.arrived(&outline);
        self.history.add(outline);
    }

    /// The changes waiting to be committed, of the entries that
    /// happen, in timeline order.
    pub(super) async fn pending<V: View>(
        &self,
        step: &Step<V>,
    ) -> Result<Vec<Pending>, StoreError> {
        let mut pending = Vec::new();
        for stamp in &self.verdicts.live {
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

    /// Lets whoever wrote `entry` know what's become of it.
    pub(super) fn tell<V: View>(&self, entry: &Outline, outcome: Outcome, step: &mut Step<V>) {
        step.tell(Told {
            clock: self.id,
            author: entry.author,
            seq: entry.link.seq,
            outcome,
        });
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
        // A dropped model takes every entry that writes to it along.
        self.verdicts.stale |= self.dropped != settled.dropped;
        self.dropped = settled.dropped;
        self.lost |= settled.lost;
        for (stamp, happened) in settled.verdicts {
            let Some(entry) = self.history.forget(stamp) else {
                continue;
            };

            if happened {
                judge::notify(self.group, &entry, true, step);
            }
            let outcome = if happened {
                Outcome::Committed
            } else {
                Outcome::Rejected
            };
            self.tell(&entry, outcome, step);
            judge::hide(self.group, &entry, step);
            judge::notify(self.group, &entry, false, step);
            self.verdicts.gone(&entry);
        }

        self.history.commit(frontier);
        self.verdicts.stale |= self.lost;
    }

    /// Lets go of whatever was written, past the last checkpoint, by
    /// anyone `writers` leaves out. That's the whole of removal: a
    /// removed contributor's history ends at the checkpoint that
    /// removed it.
    pub(super) fn cut(&mut self, writers: Writers<'_>, height: u64) {
        // An entry written under a later checkpoint was written by
        // someone let back in since. This device is only now catching
        // up to where they were stopped.
        let earlier = self.history.uncommitted.values();
        let earlier = earlier.filter(|entry| entry.under <= height);
        let authors = earlier.map(|entry| entry.author);
        let removed = authors.filter(|&author| !writers.includes(author));
        let removed: BTreeSet<Peer> = removed.collect();
        for author in removed {
            self.void(author, 0);
        }
    }

    /// Drops everything `author` wrote past `seq` that isn't committed.
    /// From here on none of it is held. What's stored of it goes a
    /// [few entries a step].
    ///
    /// [few entries a step]: Self::bury
    pub(super) fn void(&mut self, author: Peer, seq: u64) {
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
            self.verdicts.gone(&entry);
            self.condemned.insert((author, entry.link.seq), entry);
        }
    }

    /// The last entry stored past the end of each author's chain.
    fn last_condemned(&self) -> BTreeMap<Peer, u64> {
        self.condemned.keys().copied().collect()
    }

    /// Whether what's stored at this place in `author`'s chain is an
    /// entry that was dropped.
    pub(super) fn is_condemned(&self, author: Peer, seq: u64) -> bool {
        self.condemned.contains_key(&(author, seq))
    }

    /// Deletes an entry that was dropped, and what it wrote.
    fn inter<V: View>(&self, entry: &Outline, step: &mut Step<V>) {
        step.delete(keys::log(self.group, self.id, entry.author, entry.link.seq));
        judge::hide(self.group, entry, step);
        judge::notify(self.group, entry, false, step);
        self.tell(entry, Outcome::Discarded, step);
    }

    /// Deletes up to `budget` entries that were dropped. Returns how
    /// many.
    pub(super) fn bury<V: View>(&mut self, budget: usize, step: &mut Step<V>) -> usize {
        let mut buried = 0;
        while buried < budget {
            let Some((_, entry)) = self.condemned.pop_first() else {
                break;
            };
            self.inter(&entry, step);
            buried += 1;
        }

        buried
    }

    /// Deletes whatever dropped entry is stored where `author`'s chain
    /// is about to reach.
    fn make_way<V: View>(&mut self, author: Peer, seq: u64, step: &mut Step<V>) {
        if let Some(entry) = self.condemned.remove(&(author, seq)) {
            self.inter(&entry, step);
        }
    }

    /// Deletes every dropped entry stored where `frontier` says an
    /// author's chain has reached.
    fn make_way_through<V: View>(&mut self, frontier: &Frontier, step: &mut Step<V>) {
        let covered = |&(author, seq): &(Peer, u64)| seq <= reach(frontier, author);
        let covered: Vec<_> = self.condemned.keys().copied().filter(covered).collect();
        for (author, seq) in covered {
            self.make_way(author, seq, step);
        }
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

        !self.shed.iter().all(swept) || !self.condemned.is_empty()
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
            self.void(author, 0);
        }
        self.make_way_through(frontier, step);

        self.dropped = dropped;
        let covered = self.history.covered(frontier);
        let covered: Vec<_> = covered.map(Outline::stamp).collect();
        for stamp in covered {
            if let Some(entry) = self.history.forget(stamp) {
                judge::hide(self.group, &entry, step);
                self.tell(&entry, Outcome::Covered, step);
                self.verdicts.gone(&entry);
            }
        }

        self.shed_through(frontier);
        self.history.adopt(frontier);
        self.lost = false;
        self.verdicts.stale = true;
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
