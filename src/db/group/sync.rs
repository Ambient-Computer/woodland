//! Sync: telling each peer what it lacks.
//!
//! Each side says how much it holds, and from then on each sends the
//! other whatever it has that the other doesn't, as it gets it. Two
//! peers with the same things have nothing to say.
//!
//! What a peer says it holds is taken at its word, every time: it can
//! hold less than it did, having let go of what a removal cut. What
//! it's been sent since is assumed to have arrived, so nothing is sent
//! twice. That assumption is dropped on every tick, because a peer may
//! turn down what it was sent, and take it later.

use std::collections::{BTreeMap, BTreeSet};

use crate::actor::{Identity, Peer, Signed};
use crate::chain::{Electorate, Gossip};
use crate::db::ClockId;
use crate::store::{StoreError, View};

use super::super::entry::{Checked, Entry, Frontier, Link, reach};
use super::super::message::{Message, Piece, Request, Summary};
use super::super::step::Step;
use super::Group;
use super::clock::Clock;
use super::commit::covers_of;
use super::compact::Sending;
use super::sealing::Handed;

/// How many entries go in one message. A peer that's far behind gets
/// the rest as it answers.
const PAGE: usize = 64;

/// What a peer is thought to hold.
#[derive(Debug, Default)]
pub(super) struct Remote {
    /// Whether it has said anything yet. Until it has, there's no
    /// telling what it holds or wants, and it's sent only the chain.
    heard: bool,
    height: u64,
    /// The height of the latest checkpoint it has committed.
    folded: u64,
    /// Whether it lacks the checkpoint after that one.
    gap: bool,
    /// Whether it lacks the state the group starts from.
    unfounded: bool,
    /// The last entry of each chain it last said it holds.
    said: BTreeMap<ClockId, Frontier>,
    /// The last entry of each chain it's been sent since the last tick.
    sent: BTreeMap<ClockId, Frontier>,
    /// Everyone it last said it holds a fork proof against.
    proven: BTreeSet<Peer>,
    /// Everyone it's been sent a proof against since the last tick.
    told: BTreeSet<Peer>,
    /// Everyone whose request it has, or was sent since the last tick.
    asked: BTreeSet<Peer>,
    /// The clocks it wants anything of. `None` if all.
    interest: Option<BTreeSet<ClockId>>,
    /// Of those, the clocks it wants entries of over this connection.
    /// `None` if all.
    only: Option<BTreeSet<ClockId>>,
    /// The clocks this device wants entries of from it, over this
    /// connection. `None` if all it holds.
    asking: Option<BTreeSet<ClockId>>,
    /// The entry of each chain it shed and wants back next.
    pub(super) restoring: BTreeMap<ClockId, Frontier>,
    /// The height of the checkpoint it shed and wants back next.
    pub(super) lacking: Option<u64>,
    /// A secret it asked for, ready to be sent.
    owed: Option<Handed>,
    /// The secret it was last handed, so it's handed each one once.
    handed: Option<u64>,
    /// How far it says it has got with a snapshot.
    pub(super) staged: Option<(u64, Option<Vec<u8>>)>,
    /// The snapshot this device is sending it.
    pub(super) sending: Option<Sending>,
}

impl Remote {
    fn sent(&mut self, clock: ClockId, author: Peer, link: Link) {
        let sent = self.sent.entry(clock).or_default();
        if reach(sent, author) <= link.seq {
            sent.insert(author, link);
        }
    }

    /// What it was last sent of a snapshot of the state at `height`.
    pub(super) fn sent_of(&self, height: u64) -> Option<&Sending> {
        self.sending.as_ref().filter(|it| it.height == height)
    }

    /// Whether it's ready for the next piece of that snapshot: it has
    /// been sent none, or says it took the last one and there's more.
    pub(super) fn ready_for(&self, height: u64) -> bool {
        self.sent_of(height).is_none_or(|sent| {
            let wanted = Some((height, sent.upto.clone()));

            !sent.done && self.staged == wanted
        })
    }

    /// The clocks it wants, if not all.
    pub(super) fn interest(&self) -> Option<&BTreeSet<ClockId>> {
        self.interest.as_ref()
    }

    /// Whether it wants anything of `clock`.
    pub(super) fn wants(&self, clock: ClockId) -> bool {
        self.interest.as_ref().is_none_or(|it| it.contains(&clock))
    }

    /// Whether it wants entries of `clock` over this connection.
    fn takes(&self, clock: ClockId) -> bool {
        self.wants(clock) && self.only.as_ref().is_none_or(|it| it.contains(&clock))
    }

    /// It has `author`'s request: it's where this device got it.
    pub(super) fn asked(&mut self, author: Peer) {
        self.asked.insert(author);
    }

    /// Stops assuming that what it was sent arrived, and was kept.
    pub(super) fn doubt(&mut self) {
        self.sent.clear();
        self.told.clear();
        self.asked.clear();
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

    /// Asks `peer` for entries of only these clocks, for as long as
    /// it stays connected, or with `None`, of every clock held here.
    /// Returns whether it's connected.
    pub(crate) async fn ask<V: View>(
        &mut self,
        peer: Peer,
        clocks: Option<BTreeSet<ClockId>>,
        step: &mut Step<V>,
    ) -> Result<bool, StoreError> {
        let Some(remote) = self.remotes.get_mut(&peer) else {
            return Ok(false);
        };
        remote.asking = clocks;
        self.push(peer, true, false, step).await?;

        Ok(true)
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
        remote.heard = true;
        // Like what it holds, how far along it is can go down: it may
        // have let the group go and been invited back.
        remote.height = message.summary.height;
        remote.folded = message.summary.folded;
        remote.gap = message.summary.gap;
        remote.unfounded = message.summary.unfounded;
        remote.staged = message.summary.staged;
        remote.said.clone_from(&message.summary.heads);
        remote.proven.clone_from(&message.summary.proven);
        remote.interest.clone_from(&message.summary.interest);
        remote.only.clone_from(&message.summary.only);
        remote.restoring.clone_from(&message.summary.restoring);
        remote.lacking = message.summary.lacking;
        let asked = message
            .summary
            .wanting
            .filter(|it| remote.handed != Some(*it));

        self.said |= self.chain.receive(message.chain);
        self.overhear(step);
        self.hear_requests(from, message.requests);

        // What the chain now says of secrets comes before anything
        // sealed under them.
        self.unlock(identity, step).await?;
        if let Some(handed) = message.secret {
            self.take_handed(identity, handed, step).await?;
        }
        let owed = asked.and_then(|epoch| self.hand(identity, from, epoch));
        let handing = owed.is_some();
        if let (Some(remote), true) = (self.remotes.get_mut(&from), handing) {
            remote.owed = owed;
        }
        for proof in message.proofs {
            self.accuse(proof, step);
        }

        let tip = self.chain.tip();
        let writers = tip.electorate.clone();
        let decided = covers_of(tip).unwrap_or_default();

        // A peer that sent entries, or part of a snapshot, is waiting to
        // hear how far they got.
        let first = message.snapshot.as_ref().filter(|it| it.start.is_none());
        let leading = first.map(|it| it.height);
        let took = self.mend(message.links, leading) | self.take_back(message.restored, step);
        let sent = !message.entries.is_empty() || message.snapshot.is_some() || took || handing;
        let owed = sent.then_some(from);
        if let Some(piece) = message.snapshot {
            self.stage(piece, step).await?;
        }

        let check = |entry: Entry| entry.check(self.id, &self.keys);
        let wanted = |entry: &Checked| self.wants(entry.header().clock);
        // An entry names the checkpoint its author held. If another
        // was decided at that height, the one it names never was.
        let named = |entry: &Checked| {
            let under = entry.header().under;
            let decided = self.chain.at(under.height);

            decided.is_none_or(|it| it.digest() == under.checkpoint)
        };
        let entries = message.entries.into_iter().filter_map(check);
        let mut waiting: Vec<Checked> = entries.filter(wanted).filter(named).collect();

        // Whatever can't be placed may be evidence: of a fork, or that a
        // checkpoint settled on an entry other than the one held. Either
        // changes what can be placed, so it's tried again. The rounds
        // are counted so that nothing a peer sends can keep this going.
        for _ in 0..4 {
            waiting = self.place(waiting, &writers, step);
            let mut moved = false;
            for entry in &waiting {
                if let Some(proof) = self.suspect(entry, step).await? {
                    moved |= self.accuse(proof, step);
                }

                let id = entry.header().clock;
                let decided = decided.frontier(id);
                let Some(clock) = self.clocks.get_mut(&id) else {
                    continue;
                };
                if clock.history.displaces(entry, &decided) {
                    clock.void(entry.author(), 0, step);
                    moved = true;
                }
            }
            if !moved {
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

    /// Integrates every entry that follows from what's held, and
    /// returns the rest.
    fn place<V: View>(
        &mut self,
        mut waiting: Vec<Checked>,
        writers: &Electorate,
        step: &mut Step<V>,
    ) -> Vec<Checked> {
        let proven = self.proven();
        loop {
            let before = waiting.len();
            let mut unfit = Vec::new();
            for entry in waiting {
                let (author, header) = (entry.author(), entry.header());
                let id = header.clock;
                let standing =
                    self.standing(author, id, header.under.height) && !proven.contains(&author);
                let unheard = Clock::new(self.id, id);
                let clock = self.clocks.get(&id).unwrap_or(&unheard);
                let spoken = self.within_reach(id);
                let writers = writers.writers(id).except(&proven);
                let under = header.under.height;
                let stood = |peer: Peer| self.standing(peer, id, under);
                if clock
                    .history
                    .accepts(&entry, standing, writers, &spoken, stood)
                {
                    self.clock(id).integrate(&entry, step);
                } else {
                    unfit.push(entry);
                }
            }

            waiting = unfit;
            if waiting.len() == before {
                return waiting;
            }
        }
    }

    /// How far along each chain of `clock` this device could commit
    /// with the checkpoints it holds, one after another. An entry that
    /// far along is taken from an author that has no standing now,
    /// because it had some then.
    ///
    /// A device that joined partway along lacks the checkpoints in
    /// between, gets what they covered as state, and takes no such
    /// entry: it couldn't tell one from something its author made up
    /// after being removed.
    fn within_reach(&self, clock: ClockId) -> Frontier {
        let tip = self.chain.tip().height;
        let mut height = self.folded;
        while height < tip && self.chain.at(height + 1).is_some() {
            height += 1;
        }
        if height == tip {
            return self.spoken_for(clock);
        }

        let covers = self.chain.at(height).and_then(covers_of);

        covers.unwrap_or_default().frontier(clock)
    }

    /// Whether this device can't commit its way forward entry by entry:
    /// it doesn't hold the checkpoint after the last one it committed.
    fn gap(&self) -> bool {
        self.folded < self.chain.tip().height && self.chain.at(self.folded + 1).is_none()
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
        // it lacks, and one that joined partway along can't commit them
        // one checkpoint at a time. Either gets committed state instead,
        // and then the rest.
        let behind =
            remote.folded < self.shed_below(remote) || (remote.gap && remote.folded < self.folded);
        let behind = behind || remote.unfounded;
        let snapshot = if behind && remote.heard {
            self.piece(remote, step).await?
        } else {
            None
        };
        let entries = if behind || !remote.heard {
            Vec::new()
        } else {
            self.missing(remote, step).await?
        };
        let restored = self.older(remote, step).await?;
        let (snapshot, sending): (Option<Piece>, Option<_>) = snapshot.unzip();
        let height = self.chain.tip().height;

        // State from before the latest checkpoint is no use without
        // the checkpoints since: the peer commits its way forward
        // through each. They go with the first piece.
        let first = snapshot.as_ref().filter(|it| it.start.is_none());
        let lead = first.filter(|it| it.height < height).map(|first| {
            let mut since = self.chain.older(height - 1, usize::MAX);
            since.retain(|link| link.checkpoint.height >= first.height);

            since
        });
        let unasked = lead.is_some();
        let links = lead.unwrap_or_else(|| self.links(remote));
        let chain = if statements || remote.height < height {
            self.chain.gossip(remote.height)
        } else {
            Gossip::default()
        };

        // Proof against someone who's gone already changes nothing.
        let members = &self.chain.tip().electorate;
        let known = |author: &&Peer| remote.proven.contains(author) || remote.told.contains(author);
        let accused: Vec<Peer> = self
            .proofs
            .keys()
            .filter(|it| !known(it) && members.has(**it))
            .copied()
            .collect();
        let proofs = accused.iter().filter_map(|author| self.proofs.get(author));
        let proofs: Vec<_> = proofs.cloned().collect();

        let untold = |(author, _): &(&Peer, &Signed<Request>)| !remote.asked.contains(author);
        let requests = self.requests.iter().filter(untold);
        let requests =
            requests.filter(|(author, request)| self.stands(**author, request.payload()));
        let requests: Vec<_> = requests.map(|(_, request)| request.clone()).collect();

        let secret = remote.owed.clone();
        let news = !entries.is_empty()
            || secret.is_some()
            || !restored.is_empty()
            || !links.is_empty()
            || !proofs.is_empty()
            || !requests.is_empty()
            || snapshot.is_some()
            || statements
            || remote.height < height;
        if !(news || always) {
            return Ok(());
        }

        let message = Message {
            group: self.id,
            summary: Summary {
                only: remote.asking.clone(),
                ..self.summary()
            },
            chain,
            entries,
            links,
            restored,
            proofs,
            snapshot,
            requests,
            secret,
        };
        step.send(peer, &message);
        if let Some(remote) = self.remotes.get_mut(&peer) {
            if let Some(handed) = remote.owed.take() {
                remote.handed = Some(handed.epoch);
            }
            remote.height = remote.height.max(height);
            if sending.is_some() {
                remote.sending = sending;
            }
            remote.told.extend(accused);
            let authors = message.requests.iter().map(Signed::author);
            remote.asked.extend(authors);
            let asked = if unasked { &[][..] } else { &message.links[..] };
            remote.gave_back(&message.restored, asked);
            for entry in &message.entries {
                let clock = entry.header.payload().clock;
                remote.sent(clock, entry.header.author(), entry.link());
            }
        }

        Ok(())
    }

    fn summary(&self) -> Summary {
        Summary {
            height: self.chain.tip().height,
            folded: self.folded,
            gap: self.gap(),
            unfounded: self.is_unfounded(),
            staged: self.staging.as_ref().map(|it| (it.height, it.upto.clone())),
            heads: self
                .clocks
                .iter()
                .map(|(&id, clock)| (id, clock.history.heads.clone()))
                .collect(),
            proven: self.proofs.keys().copied().collect(),
            interest: self.interest.clone(),
            only: None,
            restoring: self.restoring(),
            lacking: self.lacking(),
            wanting: self.wanting(),
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
            if !remote.takes(*id) {
                continue;
            }

            let said = remote.said.get(id).unwrap_or(&nothing);
            let sent = remote.sent.get(id).unwrap_or(&nothing);
            missing.extend(clock.missing(said, sent, page, step).await?);
        }

        missing.sort_by_key(|(stamp, _)| *stamp);
        missing.truncate(PAGE);

        Ok(missing.into_iter().map(|(_, entry)| entry).collect())
    }
}
