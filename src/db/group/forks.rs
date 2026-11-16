//! Forks: noticing that a contributor signed two entries for one place,
//! keeping the proof, and letting go of what it can no longer be
//! trusted to have written once.
//!
//! A member that says two things about one checkpoint is treated the
//! same way. Its proof comes from the chain, and what follows doesn't
//! care which kind it is.
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
use super::super::proof::{Offence, Proof};
use super::super::step::Step;
use super::Group;
use super::commit::covers_of;

impl Group {
    /// Picks up the proofs this device has kept.
    pub(super) async fn recall<V: View>(&mut self, step: &Step<V>) -> Result<(), StoreError> {
        let (start, end) = keys::range(keys::proofs(self.id));
        for (_, stored) in step.scan(&start, &end).await? {
            let proof = encoding::decode::<Proof>(&stored);
            let offence = proof.as_ref().and_then(|proof| proof.check(self.id));
            let (Some(proof), Some(offence)) = (proof, offence) else {
                return Err(StoreError::new("a stored proof is unreadable"));
            };
            self.proofs.insert(offence.author(), proof);
        }

        Ok(())
    }

    /// Takes in a proof, from anyone. Returns whether it's a new one
    /// that holds.
    pub(super) fn accuse<V: View>(&mut self, proof: Proof, step: &mut Step<V>) -> bool {
        let Some(offence) = proof.check(self.id) else {
            return false;
        };
        let author = offence.author();
        if self.proofs.contains_key(&author) || !self.counts(&offence) {
            return false;
        }

        match offence {
            Offence::Fork(fork) => tracing::warn!(
                group = ?self.id,
                clock = ?fork.clock,
                ?author,
                "a contributor signed two entries for one place in its chain"
            ),
            Offence::Conflict(_) => tracing::warn!(
                group = ?self.id,
                ?author,
                "a member said two things about one checkpoint that it couldn't both mean"
            ),
        }
        step.save(keys::proof(self.id, author), &proof);
        self.proofs.insert(author, proof);
        self.silence(author, step);

        true
    }

    /// Whether an offence is one against this group, as far as this
    /// device can tell yet.
    fn counts(&self, offence: &Offence) -> bool {
        match offence {
            // Two entries at one place are a fork only if both could
            // count. A contributor that was stopped from writing here
            // in between, and let back in, writes over entries that
            // were cut. Without the checkpoints in question, there's
            // no telling yet.
            Offence::Fork(fork) => {
                let (first, last) = fork.under;
                let wrote = |it: &Electorate| it.writers(fork.clock).includes(fork.author);
                let electorates = self.chain.electorates_between(first, last);

                electorates.is_some_and(|mut electorates| electorates.all(wrote))
            }

            // Statements about a slot of some other chain, or by
            // someone whose word didn't count there, convict nobody
            // here.
            Offence::Conflict(conflict) => self.chain.concerns(conflict) == Some(true),
        }
    }

    /// Takes in whatever the chain overheard that convicts its signer.
    pub(super) fn overhear<V: View>(&mut self, step: &mut Step<V>) {
        for evidence in self.chain.evidence() {
            self.accuse(Proof::Conflict(Box::new(evidence)), step);
        }
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

        Ok(differs.then(|| Proof::fork(held.header, entry.entry.header.clone())))
    }
}
