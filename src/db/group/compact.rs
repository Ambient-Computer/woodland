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

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::actor::Peer;
use crate::db::{Changed, keys};
use crate::model::ModelId;
use crate::store::{StoreError, View};

use super::super::entry::{Frontier, Outline, reach};
use super::super::message::Piece;
use super::super::step::Step;
use super::Group;
use super::commit::covers_of;
use super::judge;
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
    /// The sum of what's been received.
    sum: Vec<u8>,
}

/// A snapshot on its way out to one peer.
#[derive(Debug)]
pub(super) struct Sending {
    height: u64,
    upto: Option<Vec<u8>>,
    done: bool,
}

impl Group {
    /// Sheds every entry a checkpoint covers, and every checkpoint that
    /// isn't needed to prove a later one.
    pub(crate) async fn compact<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        let committed = self.history.committed.clone();
        self.shed_through(&committed, step).await?;
        self.base = self.folded;
        self.chain.compact(self.folded);
        self.said = true;

        Ok(())
    }

    async fn shed_through<V: View>(
        &mut self,
        frontier: &Frontier,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        for (&author, link) in frontier {
            let from = reach(&self.shed, author) + 1;
            let start = keys::log(self.id, author, from);
            let end = keys::log(self.id, author, link.seq + 1);
            for (key, _) in step.scan(&start, &end).await? {
                step.delete(key);
            }
            if from <= link.seq {
                self.shed.insert(author, *link);
            }
        }

        Ok(())
    }

    /// The next piece of a snapshot for a peer that's behind what this
    /// device has shed, if the peer is ready for one.
    pub(super) async fn piece<V: View>(
        &self,
        remote: &Remote,
        step: &Step<V>,
    ) -> Result<Option<(Piece, Sending)>, StoreError> {
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
        let sending = Sending {
            height,
            upto: last.or_else(|| start.clone()),
            done,
        };
        let piece = Piece {
            height,
            start,
            values,
            end: done.then(|| self.dropped.clone()),
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
                    sum: Accumulator::default().to_bytes(),
                }
            }

            // Not the piece that comes next. The sender will hear where
            // this device has got to, and carry on from there.
            None => {
                self.staging = staging;
                return Ok(());
            }
        };

        let Some(mut sum) = Accumulator::from_bytes(&staging.sum) else {
            return self.unstage(step).await;
        };
        for (key, value) in piece.values {
            let Some((model, rest)) = key.split_first_chunk::<32>() else {
                return self.unstage(step).await;
            };
            if staging.after.as_ref().is_some_and(|after| *after >= key) {
                return self.unstage(step).await;
            }

            sum.hold(ModelId::from_bytes(*model), rest, &value);
            step.put(keys::staged_value(self.id, &key), value);
            staging.after = Some(key);
        }

        let Some(dropped) = piece.end else {
            staging.sum = sum.to_bytes();
            self.staging = Some(staging);
            return Ok(());
        };

        for &model in &dropped {
            sum.drop(model);
        }
        let covers = self.chain.at(staging.height).and_then(covers_of);
        let Some(covers) = covers.filter(|covers| covers.state == sum.root()) else {
            tracing::warn!(
                group = ?self.id,
                height = staging.height,
                "refused a snapshot that isn't the state its checkpoint names"
            );
            return self.unstage(step).await;
        };

        self.install(staging.height, &covers.frontier, dropped, &sum, step)
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
        frontier: &Frontier,
        dropped: BTreeSet<ModelId>,
        sum: &Accumulator,
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
        step.put(keys::state(self.id), sum.to_bytes());
        self.dropped = dropped;

        let covered = self.history.covered(frontier);
        let covered: Vec<_> = covered.map(Outline::stamp).collect();
        for stamp in covered {
            if let Some(entry) = self.history.forget(stamp) {
                judge::hide(self.id, &entry, step);
                self.live.remove(&stamp);
            }
        }

        self.shed_through(frontier, step).await?;
        self.history.adopt(frontier);
        self.folded = height;
        self.base = height;
        self.sound.clear();
        self.staging = None;

        // Catching up can skip the checkpoint that removed someone.
        let entries = self.history.uncommitted.values();
        let void = entries.filter(|entry| !self.standing(entry.author, entry.under));
        let void: BTreeSet<Peer> = void.map(|entry| entry.author).collect();
        for author in void {
            self.void(author, step);
        }

        self.stale = true;
        step.changed(Changed::Everything { group: self.id });
        tracing::info!(group = ?self.id, height, "caught up from a snapshot");

        Ok(())
    }
}
