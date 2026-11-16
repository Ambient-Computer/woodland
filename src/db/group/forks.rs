//! Forks: noticing that a contributor signed two entries for one place,
//! keeping the proof, and letting go of what it can no longer be
//! trusted to have written once.
//!
//! Nothing here tolerates a fork. A group of a few devices can't
//! outvote a liar, but it can prove one, and whoever holds the proof
//! proposes a checkpoint without it. What a checkpoint already covers
//! of the forked chain counts. Nothing else of it ever will.

use std::collections::BTreeSet;

use crate::actor::Peer;
use crate::chain::Electorate;
use crate::db::{ClockId, keys};
use crate::encoding;
use crate::store::{StoreError, View};

use super::super::entry::{Checked, Entry, Frontier, reach};
use super::super::proof::Proof;
use super::super::step::Step;
use super::Group;
use super::commit::covers_of;

impl Group {
    /// Picks up the proofs this device has kept.
    pub(super) async fn recall<V: View>(&mut self, step: &Step<V>) -> Result<(), StoreError> {
        let (start, end) = keys::range(keys::proofs(self.id));
        for (_, stored) in step.scan(&start, &end).await? {
            let proof = encoding::decode::<Proof>(&stored);
            let fork = proof.as_ref().and_then(|proof| proof.check(self.id));
            let (Some(proof), Some(fork)) = (proof, fork) else {
                return Err(StoreError::new("a stored proof is unreadable"));
            };
            self.proofs.insert(fork.author, proof);
        }

        Ok(())
    }

    /// Takes in a fork proof, from anyone. Returns whether it's a new
    /// one that holds.
    pub(super) fn accuse<V: View>(&mut self, proof: Proof, step: &mut Step<V>) -> bool {
        let Some(fork) = proof.check(self.id) else {
            return false;
        };
        if self.proofs.contains_key(&fork.author) {
            return false;
        }

        // Two entries at one place are a fork only if both could count.
        // A contributor that was stopped from writing here in between,
        // and let back in, writes over entries that were cut. Without
        // the checkpoints in question, there's no telling yet.
        let (first, last) = fork.under;
        let wrote = |it: &Electorate| it.writers(fork.clock).includes(fork.author);
        let electorates = self.chain.electorates_between(first, last);
        let counted = electorates.map(|mut electorates| electorates.all(wrote));
        if counted != Some(true) {
            return false;
        }

        tracing::warn!(
            group = ?self.id,
            clock = ?fork.clock,
            author = ?fork.author,
            "a contributor signed two entries for one place in its chain"
        );
        step.save(keys::proof(self.id, fork.author), &proof);
        self.proofs.insert(fork.author, proof);
        self.silence(fork.author, step);

        true
    }

    /// Everyone proven to have forked. What they wrote that no
    /// checkpoint covers will never count, so nothing waits for it.
    pub(super) fn proven(&self) -> BTreeSet<Peer> {
        self.proofs.keys().copied().collect()
    }

    /// How far along each chain of `clock` a checkpoint reaches: the
    /// latest one, or any that's been proposed to follow it. A device
    /// may have voted for one of those, and has to hold what it voted
    /// for until the vote comes to something or to nothing.
    pub(super) fn spoken_for(&self, clock: ClockId) -> Frontier {
        let proposed = self.chain.proposed().filter_map(covers_of);
        let mut spoken = covers_of(self.chain.tip())
            .unwrap_or_default()
            .frontier(clock);
        for covers in proposed {
            for (author, link) in covers.frontier(clock) {
                if reach(&spoken, author) < link.seq {
                    spoken.insert(author, link);
                }
            }
        }

        spoken
    }

    /// Drops whatever `author` wrote that no checkpoint covers, or has
    /// been proposed to.
    fn silence<V: View>(&mut self, author: Peer, step: &mut Step<V>) {
        let ids: Vec<ClockId> = self.clocks.keys().copied().collect();
        for id in ids {
            let kept = reach(&self.spoken_for(id), author);
            self.clock(id).void(author, kept, step);
        }
    }

    /// Does the same for everyone proven to have forked. What a
    /// proposal named and the checkpoint that was decided didn't is
    /// let go of here.
    pub(super) fn silence_all<V: View>(&mut self, step: &mut Step<V>) {
        for author in self.proven() {
            self.silence(author, step);
        }
    }

    /// Whether `entry`, which couldn't be placed, is one of two its
    /// author signed for the same place. If so, the two are the proof.
    pub(super) async fn suspect<V: View>(
        &self,
        entry: &Checked,
        step: &Step<V>,
    ) -> Result<Option<Proof>, StoreError> {
        let header = entry.header();
        let key = keys::log(self.id, header.clock, entry.author(), header.seq);
        let Some(held) = step.load::<Entry>(&key).await? else {
            return Ok(None);
        };

        let differs = held.link().hash != entry.link().hash;

        Ok(differs.then(|| Proof::new(held.header, entry.entry.header.clone())))
    }
}
