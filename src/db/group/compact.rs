//! Shedding history, and catching up without it.
//!
//! Once an entry is committed, its effect is in committed state and the
//! checkpoint that covers it names it. A device that sheds it reads
//! exactly as before. What it can no longer do is hand the entry to a
//! peer that's behind, so it hands over committed state instead: a
//! snapshot, a piece at a time.
//!
//! A snapshot is taken on nobody's word. The receiver keeps the pieces
//! to one side, adds them up as they arrive, and reads none of it until
//! the sum is the one named by a checkpoint it has verified.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::actor::Peer;
use crate::db::{Changed, ClockId, keys};
use crate::model::ModelId;
use crate::store::{StoreError, View};

use super::super::message::Piece;
use super::super::step::Step;
use super::Group;
use super::carry::Unsealed;
use super::commit::{Covers, covers_of};
use super::root::Accumulator;
use super::sync::Remote;

/// How many values go in one piece of a snapshot.
const PIECE: usize = 256;

/// A snapshot on its way in.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Staging {
    /// The checkpoint it's the state of.
    pub(super) height: u64,
    /// The last key received.
    pub(super) after: Option<Vec<u8>>,
    /// Where the last piece said it got to, as the sender put it. The
    /// next piece starts there.
    pub(super) upto: Option<Vec<u8>>,
    /// The sum of what's been received, for each clock.
    pub(super) sums: BTreeMap<ClockId, Vec<u8>>,
}

/// How many values a step moves into place.
const MOVE: usize = if cfg!(test) { 5 } else { 256 };

/// A whole, verified snapshot, on its way to being this device's
/// committed state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Installing {
    pub(super) height: u64,
    dropped: BTreeSet<ModelId>,
    sums: BTreeMap<ClockId, Vec<u8>>,
    /// Up to this key, committed state is the snapshot's.
    after: Option<Vec<u8>>,
}

/// A snapshot on its way out to one peer.
#[derive(Debug)]
pub(super) struct Sending {
    pub(super) height: u64,
    /// Where the last piece got to, as the peer was told.
    pub(super) upto: Option<Vec<u8>>,
    /// The last key that piece got to. A device passing on pieces it
    /// can't read doesn't know.
    pub(super) at: Option<Vec<u8>>,
    pub(super) done: bool,
}

impl Group {
    /// Sheds every entry a checkpoint covers, in `only` one clock or in
    /// all of them, and every checkpoint that isn't needed to prove a
    /// later one or to commit what's left.
    pub(crate) async fn compact<V: View>(
        &mut self,
        only: Option<ClockId>,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        // A device that can't read the group has no state to stand in
        // for history. It sheds what it's handed newer sealed state
        // for, and nothing else.
        if self.carries() {
            return Ok(());
        }

        // Partway through a checkpoint, what's committed is the state
        // of no checkpoint, and couldn't stand in for the history.
        if self.is_busy() {
            return Ok(());
        }

        let folded = self.folded;
        let chosen = |id: &ClockId| only.is_none_or(|only| only == *id);
        for (_, clock) in self.clocks.iter_mut().filter(|(id, _)| chosen(id)) {
            let committed = clock.history.committed.clone();
            clock.shed_through(&committed, step).await?;
            clock.base = folded;
            clock.restoring = false;
        }
        if only.is_none() {
            self.base = folded;
        }
        self.mending = false;

        // A clock that keeps its history keeps the checkpoints that
        // history is committed by.
        let bases = self.clocks.values().map(|clock| clock.base.max(self.base));
        self.chain.compact(bases.min().unwrap_or(self.base));
        self.said = true;

        Ok(())
    }

    /// The next piece of a snapshot for a peer that's behind what this
    /// device has shed, if the peer is ready for one.
    pub(super) async fn piece<V: View>(
        &self,
        remote: &Remote,
        step: &Step<V>,
    ) -> Result<Option<(Piece, Sending)>, StoreError> {
        if self.carries() {
            return self.carried_piece(remote, step).await;
        }

        // A device that holds only some clocks has the whole state of
        // nothing but those. Partway through a checkpoint it has the
        // state of none.
        if self.is_busy() || !self.serves(remote) {
            return Ok(None);
        }

        let height = self.folded;
        if !remote.ready_for(height) {
            return Ok(None);
        }
        let sent = remote.sent_of(height);
        let told = sent.and_then(|it| it.upto.clone());
        let start = sent.and_then(|it| it.at.clone());

        let prefix = keys::committed(self.id);
        let (first, end) = keys::range(prefix.clone());
        let resume = |after: &Vec<u8>| keys::after(&[&prefix[..], after].concat());
        let from = start.as_ref().map_or(first, resume);
        let mut page = step.scan_page(&from, &end, PIECE + 1).await?;
        let done = page.len() <= PIECE;
        page.truncate(PIECE);
        let value = |(key, value): (Vec<u8>, Vec<u8>)| (key[prefix.len()..].to_vec(), value);
        let values: Vec<_> = page.into_iter().map(value).collect();
        let last = values.last().map(|(key, _)| key.clone());
        let upto = last.or_else(|| start.clone());

        // A value's key starts with its model's ID, and that with its
        // clock's.
        let wanted = |(key, _): &(Vec<u8>, Vec<u8>)| {
            let clock = key.first_chunk::<32>().map(|it| ClockId::from_bytes(*it));

            clock.is_some_and(|clock| remote.wants(clock))
        };
        let values: Vec<_> = values.into_iter().filter(wanted).collect();
        let dropped = self.clocks.values().flat_map(|clock| &clock.dropped);
        let piece = Piece {
            height,
            start: if self.is_sealed() { told } else { start },
            values,
            upto: upto.clone(),
            end: done.then(|| dropped.copied().collect()),
            sealed: None,
        };
        let Some(piece) = self.seal_piece(piece) else {
            return Ok(None);
        };
        let sending = Sending {
            height,
            upto: piece.upto.clone(),
            at: upto,
            done,
        };

        Ok(Some((piece, sending)))
    }

    /// Takes in a piece of a snapshot. Once the last one is in, and the
    /// whole adds up to what the checkpoint names, it becomes this
    /// device's committed state.
    pub(super) async fn stage<V: View>(
        &mut self,
        piece: Piece,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        if piece.height <= self.folded || self.installing.is_some() {
            return Ok(());
        }
        let piece = match self.open_piece(piece) {
            Unsealed::Read(piece) => piece,
            Unsealed::Carried(piece) => return self.carry(piece, step).await,
            Unsealed::Refused => return Ok(()),
        };
        let Some(mut staging) = self.next_piece(&piece, step).await? else {
            return Ok(());
        };

        let mut sums = BTreeMap::new();
        for (&clock, sum) in &staging.sums {
            let Some(sum) = Accumulator::from_bytes(sum) else {
                return self.unstage(step).await;
            };
            sums.insert(clock, sum);
        }

        for (key, value) in piece.values {
            let Some((model, rest)) = key.split_first_chunk::<64>() else {
                return self.unstage(step).await;
            };
            if staging.after.as_ref().is_some_and(|after| *after >= key) {
                return self.unstage(step).await;
            }

            let model = ModelId::from_bytes(*model);
            if self.wants(model.clock()) {
                let sum: &mut Accumulator = sums.entry(model.clock()).or_default();
                sum.hold(model, rest, &value);
                step.put(keys::staged_value(self.id, &key), value);
            }
            staging.after = Some(key);
        }
        staging.upto = piece.upto;

        let Some(dropped) = piece.end else {
            let bytes = |(&clock, sum): (&ClockId, &Accumulator)| (clock, sum.to_bytes());
            staging.sums = sums.iter().map(bytes).collect();
            self.staging = Some(staging);
            return Ok(());
        };

        for &model in dropped.iter().filter(|model| self.wants(model.clock())) {
            sums.entry(model.clock()).or_default().drop(model);
        }

        // Every clock adds up to what the checkpoint names for it, and
        // there's nothing in a clock the checkpoint doesn't name.
        let nothing = Accumulator::default();
        let covers = self.chain.at(staging.height).and_then(covers_of);
        let matches = |covers: &Covers| {
            let named = |(id, covered): (&ClockId, _)| {
                let covered: &super::commit::Covered = covered;

                !self.wants(*id)
                    || sums.get(id).unwrap_or(&nothing).root(&self.keys) == covered.state
            };

            covers.clocks.iter().all(named) && sums.keys().all(|id| covers.clocks.contains_key(id))
        };
        let Some(covers) = covers.filter(matches) else {
            tracing::warn!(
                group = ?self.id,
                height = staging.height,
                "refused a snapshot that isn't the state its checkpoint names"
            );
            return self.unstage(step).await;
        };

        // It's all here and it's right. Moving it into place is work
        // for this step and the ones after.
        drop(covers);
        let bytes = |(&clock, sum): (&ClockId, &Accumulator)| (clock, sum.to_bytes());
        self.installing = Some(Installing {
            height: staging.height,
            dropped,
            sums: sums.iter().map(bytes).collect(),
            after: None,
        });
        self.staging = Some(staging);

        Ok(())
    }

    /// Where a snapshot on its way in stands once `piece` is taken, if
    /// `piece` is the one that comes next.
    pub(super) async fn next_piece<V: View>(
        &mut self,
        piece: &Piece,
        step: &mut Step<V>,
    ) -> Result<Option<Staging>, StoreError> {
        let staging = self.staging.take();
        let continues =
            |staging: &&Staging| staging.height == piece.height && staging.upto == piece.start;
        if let Some(staging) = staging.as_ref().filter(continues) {
            return Ok(Some(staging.clone()));
        }

        // Not the piece that comes next. The sender will hear where
        // this device has got to, and carry on from there.
        if piece.start.is_some() {
            self.staging = staging;
            return Ok(None);
        }

        self.unstage(step).await?;

        Ok(Some(Staging {
            height: piece.height,
            after: None,
            upto: None,
            sums: BTreeMap::new(),
        }))
    }

    /// Throws away whatever part of a snapshot has been received.
    pub(super) async fn unstage<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        let (start, end) = keys::range(keys::staged(self.id));
        for (key, _) in step.scan(&start, &end).await? {
            step.delete(key);
        }
        self.staging = None;

        Ok(())
    }

    /// Moves some more of a verified snapshot into place, in key order:
    /// what the snapshot has replaces what was committed, and what it
    /// hasn't goes. Once it's all moved, the snapshot is this device's
    /// committed state. Whatever the device holds that the snapshot's
    /// checkpoint covers is already in there; whatever it doesn't is
    /// judged again on top.
    pub(super) async fn install<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        let Some(mut installing) = self.installing.take() else {
            return Ok(());
        };

        let committed = keys::committed(self.id);
        let (first, end) = keys::range(committed.clone());
        let resume = |after: &Vec<u8>| keys::after(&[&committed[..], after].concat());
        let from = installing.after.as_ref().map_or(first, resume);
        let old = step.scan_page(&from, &end, MOVE).await?;
        let staged = keys::staged(self.id);
        let (start, end) = keys::range(staged.clone());
        let new = step.scan_page(&start, &end, MOVE).await?;

        // A full page may not be the last. Nothing past the end of one
        // is known, so this step goes no further than the nearer end.
        let last = |page: &[(Vec<u8>, Vec<u8>)], prefix: &[u8]| {
            let last = page.last().filter(|_| page.len() == MOVE);

            last.map(|(key, _)| key[prefix.len()..].to_vec())
        };
        let reach = match (last(&old, &committed), last(&new, &staged)) {
            (Some(old), Some(new)) => Some(old.min(new)),
            (old, new) => old.or(new),
        };
        let within = |key: &[u8]| reach.as_ref().is_none_or(|reach| key <= &reach[..]);
        for (key, _) in old {
            if within(&key[committed.len()..]) {
                step.delete(key);
            }
        }
        for (key, value) in new {
            let rest = &key[staged.len()..];
            if !within(rest) {
                continue;
            }

            // What's wanted can narrow while this is under way.
            let clock = rest.first_chunk::<32>().map(|it| ClockId::from_bytes(*it));
            if clock.is_some_and(|clock| self.wants(clock)) {
                step.put([&committed[..], rest].concat(), value);
            }
            step.delete(key);
        }
        step.changed(Changed::Everything { group: self.id });

        if reach.is_some() {
            installing.after = reach;
            self.installing = Some(installing);
            return Ok(());
        }

        let covers = self.chain.at(installing.height).and_then(covers_of);
        let covers = covers.unwrap_or_default();
        for (&id, covered) in &covers.clocks {
            if !self.wants(id) {
                self.passed |= !covered.frontier.is_empty();
                continue;
            }

            let sum = installing.sums.get(&id).cloned();
            let sum = sum.unwrap_or_else(|| Accumulator::default().to_bytes());
            step.put(keys::state(self.id, id), sum);
            let theirs = installing.dropped.iter();
            let theirs = theirs.filter(|model| model.clock() == id);
            let theirs = theirs.copied().collect();
            self.clock(id)
                .adopt(&covered.frontier, theirs, step)
                .await?;
        }

        // Whoever sent it is waiting to hear, and has more to send.
        self.caught_up(installing.height, step);
        self.said = true;

        Ok(())
    }

    /// The clocks a snapshot being moved into place has something of,
    /// that this device doesn't want.
    fn unwanted(&self) -> Vec<ClockId> {
        let installing = self.installing.as_ref();
        let covers = installing.and_then(|it| self.chain.at(it.height));
        let covers = covers.and_then(covers_of).unwrap_or_default();
        let passed = |(id, covered): &(&ClockId, &super::commit::Covered)| {
            !self.wants(**id) && !covered.frontier.is_empty()
        };

        let unwanted = covers.clocks.iter().filter(passed);
        unwanted.map(|(id, _)| *id).collect()
    }

    /// Whether a snapshot being moved into place has anything of a
    /// clock this device doesn't want. If so, there's no starting to
    /// want it.
    pub(super) fn passing(&self) -> bool {
        !self.unwanted().is_empty()
    }

    /// Takes back what a snapshot has put in place of clocks no longer
    /// wanted. Such a clock isn't held until the whole snapshot is in.
    pub(super) async fn retract<V: View>(&self, step: &mut Step<V>) -> Result<(), StoreError> {
        for clock in self.unwanted() {
            let prefix = [&keys::committed(self.id)[..], clock.as_bytes()].concat();
            let (start, end) = keys::range(prefix);
            for (key, _) in step.scan(&start, &end).await? {
                step.delete(key);
            }
        }

        Ok(())
    }

    /// Carries on from the checkpoint at `height`, now that what it
    /// covers is in place.
    pub(super) fn caught_up<V: View>(&mut self, height: u64, step: &mut Step<V>) {
        self.folded = height;
        self.base = height;
        self.folding = None;
        self.installing = None;
        self.sound.clear();
        self.staging = None;

        // Catching up can skip the checkpoint that removed someone.
        let ids: Vec<ClockId> = self.clocks.keys().copied().collect();
        for id in ids {
            let entries = self.clocks[&id].history.uncommitted.values();
            let void = entries.filter(|entry| !self.standing(entry.author, id, entry.under));
            let void: BTreeSet<Peer> = void.map(|entry| entry.author).collect();
            for author in void {
                self.clock(id).void(author, 0, step);
            }
        }

        step.changed(Changed::Everything { group: self.id });
        tracing::info!(group = ?self.id, height, "caught up from a snapshot");
    }
}
