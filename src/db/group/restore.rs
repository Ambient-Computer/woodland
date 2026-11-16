//! Restore: getting back history that was shed.
//!
//! A device that sheds a chain's entries keeps the name of the last
//! one it shed. Every entry names the one before it, so that one name
//! vouches for the whole chain below it. A device that wants its
//! history back says which entry it wants next, an archive sends that
//! one and the ones before it, latest first, and each is checked
//! against the name held before it's kept. The archive is trusted
//! with nothing.

use std::collections::BTreeMap;

use crate::actor::{Identity, Peer};
use crate::chain::Certificate;
use crate::db::{ClockId, keys};
use crate::encoding;
use crate::store::{StoreError, View};

use super::super::entry::{Checked, Entry, Frontier, Link, reach};
use super::super::step::Step;
use super::Group;
use super::clock::Clock;
use super::sync::Remote;

/// How many entries of history go in one message.
const PAGE: u64 = 64;

/// How many checkpoints go in one message.
const LINKS: usize = 16;

/// The entry before `entry` in its author's chain, as a peer that's
/// just taken `entry` would ask for it. `None` if it's the first.
fn before(entry: &Entry) -> Option<Link> {
    let header = entry.header.payload();
    let previous = header.previous.filter(|_| header.seq > 1)?;

    Some(Link {
        seq: header.seq - 1,
        lamport: 0,
        hash: previous,
    })
}

impl Clock {
    /// The entry of each chain this device wants back next.
    fn wanted_back(&self) -> Option<&Frontier> {
        (self.restoring && !self.shed.is_empty()).then_some(&self.shed)
    }

    /// Keeps an entry that was shed, if it's the one that comes next.
    fn take_back<V: View>(&mut self, entry: &Checked, step: &mut Step<V>) -> bool {
        let (author, link) = (entry.author(), entry.link());
        let next = self.shed.get(&author).filter(|_| self.restoring);
        if next.is_none_or(|next| next.seq != link.seq || next.hash != link.hash) {
            return false;
        }

        step.save(
            keys::log(self.group, self.id, author, link.seq),
            &entry.entry,
        );
        match before(&entry.entry) {
            Some(link) => self.shed.insert(author, link),
            None => self.shed.remove(&author),
        };
        self.restoring = !self.shed.is_empty();

        true
    }

    /// The entries a peer `wants` back that are held here, latest first
    /// in each chain, and no more than `page` of them.
    async fn older<V: View>(
        &self,
        wants: &Frontier,
        mut page: u64,
        step: &Step<V>,
    ) -> Result<Vec<Entry>, StoreError> {
        let mut older = Vec::new();
        for (&author, link) in wants {
            let held = reach(&self.shed, author) + 1..=reach(&self.history.committed, author);
            if page == 0 || !held.contains(&link.seq) {
                continue;
            }

            let first = (*held.start()).max(link.seq.saturating_sub(page - 1));
            let start = keys::log(self.group, self.id, author, first);
            let end = keys::log(self.group, self.id, author, link.seq + 1);
            let mut found = step.scan(&start, &end).await?;
            found.reverse();
            for (_, stored) in found {
                let Some(entry) = encoding::decode::<Entry>(&stored) else {
                    return Err(StoreError::new("a stored entry is unreadable"));
                };

                older.push(entry);
                page -= 1;
            }
        }

        Ok(older)
    }
}

impl Group {
    /// Starts getting back the history of `clock` that was shed here.
    /// Returns whether there's such a clock.
    pub(crate) async fn restore<V: View>(
        &mut self,
        identity: &Identity,
        clock: ClockId,
        step: &mut Step<V>,
    ) -> Result<bool, StoreError> {
        let Some(held) = self.clocks.get_mut(&clock) else {
            return Ok(false);
        };

        held.restoring = !held.shed.is_empty();
        self.mending = true;
        self.progress(identity, None, step).await?;
        let peers: Vec<Peer> = self.remotes.keys().copied().collect();
        for peer in peers {
            self.push(peer, true, false, step).await?;
        }

        Ok(true)
    }

    /// Whether every entry of `clock` ever committed is held here,
    /// with every checkpoint that committed them.
    pub(crate) fn is_whole(&self, clock: ClockId) -> bool {
        let entries = self.clocks.get(&clock).is_some_and(|it| it.shed.is_empty());

        entries && self.chain.lacks().is_none()
    }

    /// The checkpoint this device wants back next.
    pub(super) fn lacking(&self) -> Option<u64> {
        self.chain.lacks().filter(|_| self.mending)
    }

    /// Keeps whichever of `links` are checkpoints this device shed
    /// and asked for, or that lead up from a snapshot at `leading`
    /// that came with them. Returns whether any were.
    pub(super) fn mend(&mut self, links: Vec<Certificate>, leading: Option<u64>) -> bool {
        let mut mended = false;
        for link in links {
            let leads = leading.is_some_and(|height| link.checkpoint.height >= height);
            mended |= (self.mending || leads) && self.chain.fill(link);
        }
        self.mending &= self.chain.lacks().is_some();
        self.said |= mended;

        mended
    }

    /// The checkpoints `remote` asked for that are held here.
    pub(super) fn links(&self, remote: &Remote) -> Vec<Certificate> {
        let older = |height| self.chain.older(height, LINKS);

        remote.lacking.map(older).unwrap_or_default()
    }

    /// The entries this device wants back next, in each clock.
    pub(super) fn restoring(&self) -> BTreeMap<ClockId, Frontier> {
        let wanted = |(&id, clock): (&ClockId, &Clock)| Some((id, clock.wanted_back()?.clone()));

        self.clocks.iter().filter_map(wanted).collect()
    }

    /// Keeps whichever of `entries` are history this device asked for.
    /// Returns whether any were.
    pub(super) fn take_back<V: View>(&mut self, entries: Vec<Entry>, step: &mut Step<V>) -> bool {
        let mut took = false;
        for entry in entries {
            let Some(entry) = entry.check(self.id, &self.keys) else {
                continue;
            };
            let Some(clock) = self.clocks.get_mut(&entry.header().clock) else {
                continue;
            };

            took |= clock.take_back(&entry, step);
        }

        took
    }

    /// History `remote` asked for that's held here.
    pub(super) async fn older<V: View>(
        &self,
        remote: &Remote,
        step: &Step<V>,
    ) -> Result<Vec<Entry>, StoreError> {
        let mut older = Vec::new();
        for (id, wants) in &remote.restoring {
            let Some(clock) = self.clocks.get(id) else {
                continue;
            };

            let left = PAGE.saturating_sub(u64::try_from(older.len()).unwrap_or(PAGE));
            older.extend(clock.older(wants, left, step).await?);
        }

        Ok(older)
    }
}

impl Remote {
    /// Assumes `entries` arrived, so the same history isn't sent again
    /// before the peer says where it has got to.
    pub(super) fn gave_back(&mut self, entries: &[Entry], links: &[Certificate]) {
        if let Some(last) = links.last() {
            self.lacking = last.checkpoint.height.checked_sub(1).filter(|it| *it > 0);
        }

        for entry in entries {
            let header = entry.header.payload();
            let Some(wants) = self.restoring.get_mut(&header.clock) else {
                continue;
            };

            match before(entry) {
                Some(link) => wants.insert(entry.header.author(), link),
                None => wants.remove(&entry.header.author()),
            };
        }
    }
}
