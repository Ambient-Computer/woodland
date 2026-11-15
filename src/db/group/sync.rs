//! Sync: telling each peer what it lacks.
//!
//! Each side says how much it holds, and from then on each sends the
//! other whatever it has that the other doesn't, as it gets it. What a
//! peer is thought to hold only ever grows, so nothing is sent to it
//! twice, and two peers with the same things have nothing to say.

use crate::actor::{Identity, Peer};
use crate::chain::{Digest, Gossip};
use crate::clock::Stamp;
use crate::db::keys;
use crate::encoding;
use crate::store::{StoreError, View};

use super::super::entry::{Checked, Entry, Frontier, Link, reach};
use super::super::message::{Message, Summary};
use super::super::step::Step;
use super::Group;
use super::commit::frontier_of;

/// How many entries go in one message. A peer that's far behind gets
/// the rest as it answers.
const PAGE: usize = 64;

/// What a peer is thought to hold: what it said, what it sent, and what
/// it was sent.
#[derive(Debug, Default)]
pub(super) struct Remote {
    height: u64,
    heads: Frontier,
}

impl Remote {
    fn holds(&mut self, author: Peer, link: Link) {
        if reach(&self.heads, author) <= link.seq {
            self.heads.insert(author, link);
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
        for (&author, &link) in &message.summary.heads {
            remote.holds(author, link);
        }
        for entry in &message.entries {
            remote.holds(entry.header.author(), entry.link());
        }

        self.said |= self.chain.receive(message.chain);
        let tip = self.chain.tip();
        let writers = tip.electorate.clone();
        let decided = frontier_of(tip).unwrap_or_default();

        // A peer that sent entries is waiting to hear how far they got.
        let owed = (!message.entries.is_empty()).then_some(from);
        let check = |entry: Entry| {
            let entry = entry.check(self.id)?;
            let standing = self.standing(entry.author(), entry.header().checkpoint);

            Some((entry, standing))
        };
        let mut waiting: Vec<(Checked, bool)> =
            message.entries.into_iter().filter_map(check).collect();
        loop {
            let before = waiting.len();
            let mut unfit = Vec::new();
            for (entry, standing) in waiting {
                if self.history.accepts(&entry, standing, &writers, &decided) {
                    self.integrate(&entry, step);
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

    /// Whether `author` has been a contributor in every checkpoint since
    /// the one named `since`. If not, whatever it wrote under that
    /// checkpoint was cut when it was removed, and coming back doesn't
    /// bring it back.
    fn standing(&self, author: Peer, since: Digest) -> bool {
        let checkpoints = self.chain.checkpoints();
        let mut later = checkpoints.skip_while(|it| it.digest() != since).peekable();

        later.peek().is_some() && later.all(|it| it.electorate.includes(author))
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

        let entries = self.missing(remote, step).await?;
        let height = self.chain.tip().height;
        let chain = if statements || remote.height < height {
            self.chain.gossip(remote.height)
        } else {
            Gossip::default()
        };

        let news = !entries.is_empty() || statements || remote.height < height;
        if !(news || always) {
            return Ok(());
        }

        let message = Message {
            group: self.id,
            summary: self.summary(),
            chain,
            entries,
        };
        step.send(peer, &message);
        if let Some(remote) = self.remotes.get_mut(&peer) {
            remote.height = remote.height.max(height);
            for entry in &message.entries {
                remote.holds(entry.header.author(), entry.link());
            }
        }

        Ok(())
    }

    fn summary(&self) -> Summary {
        Summary {
            height: self.chain.tip().height,
            heads: self.history.heads.clone(),
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
        let mut missing: Vec<(Stamp, Entry)> = Vec::new();
        for (&author, head) in &self.history.heads {
            let held = self.history.shared(author, remote.heads.get(&author));
            if held >= head.seq {
                continue;
            }

            let last = head.seq.min(held.saturating_add(page));
            let start = keys::log(self.id, author, held + 1);
            let end = keys::log(self.id, author, last + 1);
            for (_, stored) in step.scan(&start, &end).await? {
                let Some(entry) = encoding::decode::<Entry>(&stored) else {
                    return Err(StoreError::new("a stored entry is unreadable"));
                };

                let lamport = entry.header.payload().lamport;
                missing.push((Stamp { lamport, author }, entry));
            }
        }

        missing.sort_by_key(|(stamp, _)| *stamp);
        missing.truncate(PAGE);

        Ok(missing.into_iter().map(|(_, entry)| entry).collect())
    }
}
