//! Sync: telling each peer what it lacks.
//!
//! Each side says how much it holds, and from then on each sends the
//! other whatever it has that the other doesn't, as it gets it. What a
//! peer is thought to hold only ever grows, so nothing is sent to it
//! twice, and two peers with the same things have nothing to say.

use std::collections::BTreeMap;

use crate::actor::{Identity, Peer};
use crate::chain::{Electorate, Gossip};
use crate::db::ClockId;
use crate::store::{StoreError, View};

use super::super::entry::{Checked, Entry, Frontier, Link, reach};
use super::super::message::{Message, Summary};
use super::super::step::Step;
use super::Group;
use super::clock::Clock;
use super::commit::covers_of;
use super::compact::Sending;

/// How many entries go in one message. A peer that's far behind gets
/// the rest as it answers.
const PAGE: usize = 64;

/// What a peer is thought to hold: what it said, what it sent, and what
/// it was sent.
#[derive(Debug, Default)]
pub(super) struct Remote {
    height: u64,
    /// The height of the latest checkpoint it has committed.
    folded: u64,
    heads: BTreeMap<ClockId, Frontier>,
    /// How far it says it has got with a snapshot.
    pub(super) staged: Option<(u64, Option<Vec<u8>>)>,
    /// The snapshot this device is sending it.
    pub(super) sending: Option<Sending>,
}

impl Remote {
    fn holds(&mut self, clock: ClockId, author: Peer, link: Link) {
        let heads = self.heads.entry(clock).or_default();
        if reach(heads, author) <= link.seq {
            heads.insert(author, link);
        }
    }
}

impl Group {
    /// A peer can be reached. Tells it where this device stands.
    pub(crate) async fn connected<V: View>(
        &mut self,
        peer: Peer,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        self.remotes.insert(peer, Remote::default());

        self.push(peer, true, true, step).await
    }

    pub(crate) fn disconnected(&mut self, peer: Peer) {
        self.remotes.remove(&peer);
    }

    pub(crate) async fn receive<V: View>(
        &mut self,
        identity: &Identity,
        from: Peer,
        message: Message,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let remote = self.remotes.entry(from).or_default();
        remote.height = remote.height.max(message.summary.height);
        remote.folded = remote.folded.max(message.summary.folded);
        remote.staged = message.summary.staged;
        for (&clock, heads) in &message.summary.heads {
            for (&author, &link) in heads {
                remote.holds(clock, author, link);
            }
        }
        for entry in &message.entries {
            let clock = entry.header.payload().clock;
            remote.holds(clock, entry.header.author(), entry.link());
        }

        self.said |= self.chain.receive(message.chain);
        let tip = self.chain.tip();
        let writers = tip.electorate.clone();
        let decided = covers_of(tip).unwrap_or_default();

        // A peer that sent entries, or part of a snapshot, is waiting to
        // hear how far they got.
        let sent = !message.entries.is_empty() || message.snapshot.is_some();
        let owed = sent.then_some(from);
        if let Some(piece) = message.snapshot {
            self.stage(piece, step).await?;
        }

        let check = |entry: Entry| {
            let entry = entry.check(self.id)?;
            let header = entry.header();
            let standing = self.standing(entry.author(), header.clock, header.under.height);

            Some((entry, standing))
        };
        let mut waiting: Vec<(Checked, bool)> =
            message.entries.into_iter().filter_map(check).collect();
        loop {
            let before = waiting.len();
            let mut unfit = Vec::new();
            for (entry, standing) in waiting {
                let id = entry.header().clock;
                let unheard = Clock::new(self.id, id);
                let clock = self.clocks.get(&id).unwrap_or(&unheard);
                let decided = decided.frontier(id);
                if clock
                    .history
                    .accepts(&entry, standing, writers.writers(id), &decided)
                {
                    self.clock(id).integrate(&entry, step);
                } else {
                    unfit.push((entry, standing));
                }
            }

            waiting = unfit;
            if waiting.len() == before {
                break;
            }
        }

        if !waiting.is_empty() {
            tracing::debug!(
                group = ?self.id,
                peer = ?from,
                entries = waiting.len(),
                "set aside entries that don't follow from what's held"
            );
        }

        self.progress(identity, owed, step).await
    }

    /// Whether `author` has been allowed to write to `clock` ever since
    /// the checkpoint at `height`. If not, whatever it wrote there under
    /// that checkpoint was cut when it was stopped, and being let back
    /// in doesn't bring it back.
    pub(super) fn standing(&self, author: Peer, clock: ClockId, height: u64) -> bool {
        let mut electorates = self.chain.electorates_since(height);
        let wrote = |it: &Electorate| it.writers(clock).includes(author);

        height <= self.chain.tip().height && electorates.all(wrote)
    }

    /// Sends `peer` what it's thought to lack. With `always`, sends even
    /// if that's nothing. With `statements`, includes what's been said
    /// about the next checkpoint.
    pub(super) async fn push<V: View>(
        &mut self,
        peer: Peer,
        always: bool,
        statements: bool,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let Some(remote) = self.remotes.get(&peer) else {
            return Ok(());
        };

        // A peer behind what's been shed here can't be sent the entries
        // it lacks. It gets committed state instead, and then the rest.
        let behind = remote.folded < self.base;
        let snapshot = if behind {
            self.piece(remote, step).await?
        } else {
            None
        };
        let entries = if behind {
            Vec::new()
        } else {
            self.missing(remote, step).await?
        };
        let (snapshot, sending) = snapshot.unzip();
        let height = self.chain.tip().height;
        let chain = if statements || remote.height < height {
            self.chain.gossip(remote.height)
        } else {
            Gossip::default()
        };

        let news =
            !entries.is_empty() || snapshot.is_some() || statements || remote.height < height;
        if !(news || always) {
            return Ok(());
        }

        let message = Message {
            group: self.id,
            summary: self.summary(),
            chain,
            entries,
            snapshot,
        };
        step.send(peer, &message);
        if let Some(remote) = self.remotes.get_mut(&peer) {
            remote.height = remote.height.max(height);
            if sending.is_some() {
                remote.sending = sending;
            }
            for entry in &message.entries {
                let clock = entry.header.payload().clock;
                remote.holds(clock, entry.header.author(), entry.link());
            }
        }

        Ok(())
    }

    fn summary(&self) -> Summary {
        Summary {
            height: self.chain.tip().height,
            folded: self.folded,
            staged: self
                .staging
                .as_ref()
                .map(|it| (it.height, it.after.clone())),
            heads: self
                .clocks
                .iter()
                .map(|(&id, clock)| (id, clock.history.heads.clone()))
                .collect(),
        }
    }

    /// The first entries, in timeline order, that `remote` lacks. An
    /// entry's stamp is greater than that of anything it was written on
    /// top of, so in this order each one follows from those before it.
    async fn missing<V: View>(
        &self,
        remote: &Remote,
        step: &Step<V>,
    ) -> Result<Vec<Entry>, StoreError> {
        let page = u64::try_from(PAGE).unwrap_or(u64::MAX);
        let nothing = Frontier::new();
        let mut missing = Vec::new();
        for (id, clock) in &self.clocks {
            let theirs = remote.heads.get(id).unwrap_or(&nothing);
            missing.extend(clock.missing(theirs, page, step).await?);
        }

        missing.sort_by_key(|(stamp, _)| *stamp);
        missing.truncate(PAGE);

        Ok(missing.into_iter().map(|(_, entry)| entry).collect())
    }
}
