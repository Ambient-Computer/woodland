//! What a device holds of everyone's chains, and how much of it is
//! committed.

use std::collections::BTreeMap;

use crate::actor::Peer;
use crate::chain::Electorate;
use crate::clock::Stamp;

use super::super::entry::{Checked, Frontier, Hash, Link, Outline, reach};

#[derive(Debug, Default)]
pub(super) struct History {
    /// The last entry held of each author's chain.
    pub(super) heads: Frontier,
    /// The last entry of each author's chain that a folded checkpoint
    /// covers.
    pub(super) committed: Frontier,
    /// Every entry held that isn't committed, in timeline order.
    pub(super) uncommitted: BTreeMap<Stamp, Outline>,
    /// Where each uncommitted entry sits in its author's chain.
    places: BTreeMap<(Peer, u64), Stamp>,
}

impl History {
    pub(super) fn resume(heads: Frontier, committed: Frontier) -> Self {
        Self {
            heads,
            committed,
            ..Self::default()
        }
    }

    /// The greatest logical time of anything held.
    pub(super) fn lamport(&self) -> u64 {
        let links = self.heads.values().chain(self.committed.values());

        links.map(|link| link.lamport).max().unwrap_or(0)
    }

    /// Which entry sits at `seq` in `author`'s chain, if it can be told.
    /// Committed entries aren't remembered one by one.
    fn link_at(&self, author: Peer, seq: u64) -> Option<Link> {
        let uncommitted = || {
            let stamp = self.places.get(&(author, seq))?;

            Some(self.uncommitted.get(stamp)?.link)
        };
        let committed = self.committed.get(&author).filter(|link| link.seq == seq);

        committed.copied().or_else(uncommitted)
    }

    /// Whether `entry` is the next one of its author's chain, written on
    /// top of things this device holds, by someone allowed to write.
    /// `standing` is whether its author has been a contributor ever
    /// since the checkpoint the entry was written under. `decided` is
    /// what the latest checkpoint covers: an author that has since been
    /// removed still wrote those.
    pub(super) fn accepts(
        &self,
        entry: &Checked,
        standing: bool,
        writers: &Electorate,
        decided: &Frontier,
    ) -> bool {
        let (author, header) = (entry.author(), entry.header());
        let last = self.heads.get(&author);
        let next = header.seq == reach(&self.heads, author) + 1
            && header.previous == last.map(|link| link.hash)
            && last.is_none_or(|link| link.lamport < header.lamport);
        let allowed = standing || header.seq <= reach(decided, author);

        // Whatever a removed author wrote past its last checkpoint is
        // void, so nothing waits for it.
        let held = |(&other, link): (&Peer, &Link)| {
            let known = self.link_at(other, link.seq);
            let same = known.is_none_or(|known| known.hash == link.hash);
            let held = reach(&self.heads, other) >= link.seq && same;

            other != author && link.lamport < header.lamport && (held || !writers.includes(other))
        };

        next && allowed && header.after.iter().all(held)
    }

    pub(super) fn add(&mut self, outline: Outline) {
        self.heads.insert(outline.author, outline.link);
        self.remember(outline);
    }

    /// Takes back an uncommitted entry found in storage.
    pub(super) fn remember(&mut self, outline: Outline) {
        self.places
            .insert((outline.author, outline.link.seq), outline.stamp());
        self.uncommitted.insert(outline.stamp(), outline);
    }

    pub(super) fn forget(&mut self, stamp: Stamp) -> Option<Outline> {
        let outline = self.uncommitted.remove(&stamp)?;
        self.places.remove(&(outline.author, outline.link.seq));

        Some(outline)
    }

    /// Whether a checkpoint could cover exactly `frontier`: it reaches
    /// at least as far as what's committed, this device holds every
    /// entry it names, and nothing it covers was written on top of
    /// something it leaves out.
    pub(super) fn supports(&self, frontier: &Frontier, writers: &Electorate) -> bool {
        let keeps = |(author, link): (&Peer, &Link)| {
            frontier.get(author).is_some_and(|it| it.seq >= link.seq)
        };
        let holds = |(&author, link): (&Peer, &Link)| self.link_at(author, link.seq) == Some(*link);
        let closed = |outline: &Outline| {
            let within = |(&other, link): (&Peer, &Link)| {
                reach(frontier, other) >= link.seq || !writers.includes(other)
            };

            outline.after.iter().all(within)
        };

        self.committed.iter().all(keeps)
            && frontier.iter().all(holds)
            && self.covered(frontier).all(closed)
    }

    /// Whether this device holds everything `frontier` names.
    pub(super) fn holds(&self, frontier: &Frontier) -> bool {
        let held = |(&author, link): (&Peer, &Link)| reach(&self.heads, author) >= link.seq;

        frontier.iter().all(held)
    }

    /// Whether an entry this device holds isn't the one `frontier` names
    /// for its place: its author signed two.
    pub(super) fn contradicts(&self, frontier: &Frontier) -> bool {
        let differs = |(&author, link): (&Peer, &Link)| {
            let known = self.link_at(author, link.seq);

            known.is_some_and(|known| known.hash != link.hash)
        };

        frontier.iter().any(differs)
    }

    /// The uncommitted entries `frontier` covers, in timeline order.
    pub(super) fn covered<'a>(
        &'a self,
        frontier: &'a Frontier,
    ) -> impl Iterator<Item = &'a Outline> {
        let covered = |outline: &&Outline| outline.link.seq <= reach(frontier, outline.author);

        self.uncommitted.values().filter(covered)
    }

    /// Makes `frontier` what's committed. Its entries must have been
    /// forgotten already.
    pub(super) fn commit(&mut self, frontier: &Frontier) {
        for (&author, &link) in frontier {
            if reach(&self.committed, author) < link.seq {
                self.committed.insert(author, link);
            }
        }
    }

    /// Lets go of everything `author` wrote that isn't committed.
    pub(super) fn cut(&mut self, author: Peer) -> Vec<Outline> {
        let theirs = |outline: &&Outline| outline.author == author;
        let stamps: Vec<Stamp> = self
            .uncommitted
            .values()
            .filter(theirs)
            .map(Outline::stamp)
            .collect();
        match self.committed.get(&author) {
            Some(&link) => self.heads.insert(author, link),
            None => self.heads.remove(&author),
        };

        stamps
            .into_iter()
            .filter_map(|stamp| self.forget(stamp))
            .collect()
    }

    /// How much of `author`'s chain a peer shares with this device, if
    /// `theirs` is the last entry of it the peer holds. A peer holding a
    /// different entry at that place holds one that was cut, and shares
    /// only what's committed.
    pub(super) fn shared(&self, author: Peer, theirs: Option<&Link>) -> u64 {
        let Some(theirs) = theirs else {
            return 0;
        };

        let ours = self.link_at(author, theirs.seq);
        if ours.is_some_and(|ours| ours.hash != theirs.hash) {
            return reach(&self.committed, author).min(theirs.seq);
        }

        theirs.seq
    }

    pub(super) fn hash_of_head(&self, author: Peer) -> Option<Hash> {
        self.heads.get(&author).map(|link| link.hash)
    }
}
