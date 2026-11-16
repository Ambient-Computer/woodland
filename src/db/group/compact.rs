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
    /// The sum of what's been received, for each clock.
    sums: BTreeMap<ClockId, Vec<u8>>,
}

/// A snapshot on its way out to one peer.
#[derive(Debug)]
pub(super) struct Sending {
    height: u64,
    upto: Option<Vec<u8>>,
    done: bool,
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
        // A device that holds only some clocks has the whole state of
        // nothing but those.
        if !self.serves(remote) {
            return Ok(None);
        }

        let height = self.folded;
        let start = match &remote.sending {
            Some(sent) if sent.height == height => {
                let wanted = Some((height, sent.upto.clone()));
                if sent.done || remote.staged != wanted {
                    return Ok(None);
                }

                sent.upto.clone()
            }

            _ => None,
        };

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
        let sending = Sending {
            height,
            upto: upto.clone(),
            done,
        };

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
            start,
            values,
            upto,
            end: done.then(|| dropped.copied().collect()),
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
        if piece.height <= self.folded {
            return Ok(());
        }

        let staging = self.staging.take();
        let continues =
            |staging: &&Staging| staging.height == piece.height && staging.after == piece.start;
        let mut staging = match staging.as_ref().filter(continues) {
            Some(staging) => staging.clone(),
            None if piece.start.is_none() => {
                self.unstage(step).await?;

                Staging {
                    height: piece.height,
                    after: None,
                    sums: BTreeMap::new(),
                }
            }

            // Not the piece that comes next. The sender will hear where
            // this device has got to, and carry on from there.
            None => {
                self.staging = staging;
                return Ok(());
            }
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
        if piece.upto > staging.after {
            staging.after = piece.upto;
        }

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

                !self.wants(*id) || sums.get(id).unwrap_or(&nothing).root() == covered.state
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

        self.install(staging.height, &covers, &dropped, &sums, step)
            .await
    }

    /// Throws away whatever part of a snapshot has been received.
    async fn unstage<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        let (start, end) = keys::range(keys::staged(self.id));
        for (key, _) in step.scan(&start, &end).await? {
            step.delete(key);
        }
        self.staging = None;

        Ok(())
    }

    /// Makes a verified snapshot this device's committed state. Whatever
    /// it holds that the snapshot's checkpoint covers is already in
    /// there; whatever it doesn't is judged again on top.
    async fn install<V: View>(
        &mut self,
        height: u64,
        covers: &Covers,
        dropped: &BTreeSet<ModelId>,
        sums: &BTreeMap<ClockId, Accumulator>,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let (start, end) = keys::range(keys::committed(self.id));
        for (key, _) in step.scan(&start, &end).await? {
            step.delete(key);
        }

        let staged = keys::staged(self.id);
        let (start, end) = keys::range(staged.clone());
        for (key, value) in step.scan(&start, &end).await? {
            let committed = [&keys::committed(self.id)[..], &key[staged.len()..]].concat();
            step.put(committed, value);
            step.delete(key);
        }

        let nothing = Accumulator::default();
        for (&id, covered) in &covers.clocks {
            if !self.wants(id) {
                self.passed |= !covered.frontier.is_empty();
                continue;
            }

            let sum = sums.get(&id).unwrap_or(&nothing);
            step.put(keys::state(self.id, id), sum.to_bytes());
            let theirs = dropped.iter().filter(|model| model.clock() == id);
            let theirs = theirs.copied().collect();
            self.clock(id)
                .adopt(&covered.frontier, theirs, step)
                .await?;
        }

        self.folded = height;
        self.base = height;
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

        Ok(())
    }
}
