//! The checkpoint chain: how a group agrees on one sequence of
//! checkpoints, each final, each naming the one before it.
//!
//! # What it promises
//!
//! **No two checkpoints are ever decided for the same slot.** Every
//! device that holds a chain holds a prefix of the same one, so "what is
//! final" has one answer, and an order.
//!
//! That's the difference from lattice agreement, which promises values
//! that include each other, not a sequence of them. A sequence is what
//! ordering needs: something has to say that everything outside a
//! checkpoint comes after it.
//!
//! # How it works
//!
//! Each slot is decided once, by the electorate the checkpoint before it
//! named. That's what makes changing the electorate simple: there's
//! never a moment when two electorates are in play, because the next one
//! takes over exactly where the last one's decision ends.
//!
//! Deciding a slot is single-decree Paxos, said in signed statements:
//!
//! 1. **Prepare.** Someone who wants to fill the slot picks a [`Ballot`]
//!    higher than any it has seen.
//! 2. **Promise.** An elector that sees a ballot higher than any it has
//!    promised, promises it: no votes in earlier ballots, and here's the
//!    last thing it voted for.
//! 3. **Propose.** With promises from a quorum, the proposer names the
//!    checkpoint to vote for. If any promise came with an earlier vote,
//!    it must be that checkpoint (the latest one). It only gets its own
//!    way if nobody had voted yet. That's the whole of safety: a
//!    checkpoint that a quorum might have voted for is the only one a
//!    later ballot can carry.
//! 4. **Vote.** An elector votes for the proposal of the highest ballot
//!    it hasn't promised against.
//! 5. **Decide.** Votes from a quorum, for one checkpoint in one ballot,
//!    are a [`Certificate`]. Anyone holding it has the next link.
//!
//! # Where it departs from the textbook
//!
//! There are no requests and no replies. A replica holds the latest
//! statement of each kind from each elector, passes all of them on, and
//! acts on whatever it holds ([`Chain::advance`]). So a statement counts
//! however old it is and whoever relayed it, electors that never meet
//! can decide through a device that has no say, and an elector can
//! finish what another started.
//!
//! # What it needs
//!
//! - **Safety** holds whoever is offline, and however statements are
//!   delayed, dropped, repeated or reordered.
//! - **Durable promises.** An elector must persist a promise or a vote
//!   before anyone hears it. One that forgets can vote against what it
//!   promised.
//! - **Liveness** needs a quorum to hear each other, and proposers to
//!   stand back long enough for one of them to finish. Rivals who keep
//!   outbidding each other decide nothing, so whoever drives this waits
//!   before trying again.
//! - **Honest electors.** A proposal isn't checked against the promises
//!   that justify it. An elector that lies can break agreement, though
//!   not without signing two statements that prove it.

mod electorate;
mod statement;

use std::collections::{BTreeMap, BTreeSet};

pub(crate) use electorate::Electorate;
pub(crate) use statement::{
    Ballot, Certificate, Checkpoint, Digest, Prepare, Promise, Proposal, Stated, Vote,
};

use serde::{Deserialize, Serialize};

use crate::actor::{Identity, Peer, Signable, Signed, Verified, verify_all};

/// One device's replica of one chain.
#[derive(Debug)]
pub(crate) struct Chain {
    founding: Checkpoint,
    /// Every link after the founding, oldest first, each with its proof.
    decided: Vec<Certificate>,
    /// What's been said about the link after the tip.
    slot: Slot,
}

/// The slot after the tip: what this device has promised, and the latest
/// statement of each kind it holds from each elector.
#[derive(Debug, Default)]
struct Slot {
    /// The highest ballot this device promised. It votes in nothing lower.
    promised: Option<Ballot>,
    /// What this device last voted for.
    accepted: Option<(Ballot, Checkpoint)>,
    /// This device's own attempt, and the checkpoint it would like.
    candidate: Option<(Ballot, Checkpoint)>,
    prepares: BTreeMap<Peer, Verified<Prepare>>,
    promises: BTreeMap<Peer, Verified<Promise>>,
    proposals: BTreeMap<Peer, Verified<Proposal>>,
    votes: BTreeMap<Peer, Verified<Vote>>,
}

/// A replica, as it's kept in storage.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Saved {
    founding: Checkpoint,
    promised: Option<Ballot>,
    accepted: Option<(Ballot, Checkpoint)>,
    candidate: Option<(Ballot, Checkpoint)>,
    gossip: Gossip,
}

/// Everything a replica holds that another might not.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Gossip {
    pub(crate) decided: Vec<Certificate>,
    pub(crate) prepares: Vec<Signed<Prepare>>,
    pub(crate) promises: Vec<Signed<Promise>>,
    pub(crate) proposals: Vec<Signed<Proposal>>,
    pub(crate) votes: Vec<Signed<Vote>>,
}

impl Chain {
    pub(crate) fn new(founding: Checkpoint) -> Self {
        Self {
            founding,
            decided: Vec::new(),
            slot: Slot::default(),
        }
    }

    /// The latest checkpoint this device knows to be final.
    pub(crate) fn tip(&self) -> &Checkpoint {
        self.decided
            .last()
            .map_or(&self.founding, |link| &link.checkpoint)
    }

    /// Every checkpoint from the founding to the tip.
    pub(crate) fn checkpoints(&self) -> impl Iterator<Item = &Checkpoint> {
        let decided = self.decided.iter().map(|link| &link.checkpoint);

        std::iter::once(&self.founding).chain(decided)
    }

    /// Every checkpoint someone has proposed for the slot after the tip.
    pub(crate) fn proposed(&self) -> impl Iterator<Item = &Checkpoint> {
        let proposals = self.slot.proposals.values();

        proposals.map(|proposal| &proposal.payload().checkpoint)
    }

    /// The highest ballot anyone has mentioned for the slot after the
    /// tip, if an attempt is under way.
    pub(crate) fn attempt(&self) -> Option<u64> {
        let round = self.slot.highest_round();

        (round > 0).then_some(round)
    }

    /// Everything worth keeping across a restart.
    pub(crate) fn save(&self) -> Saved {
        let gossip = self.gossip(0);

        Saved {
            founding: self.founding.clone(),
            promised: self.slot.promised,
            accepted: self.slot.accepted.clone(),
            candidate: self.slot.candidate.clone(),
            gossip,
        }
    }

    /// Picks up from what was saved. It was all checked before it was
    /// stored.
    pub(crate) fn restore(saved: Saved) -> Self {
        fn trusted<T>(stored: Vec<Signed<T>>) -> BTreeMap<Peer, Verified<T>> {
            let stored = stored.into_iter().map(Verified::trust_stored);

            stored.map(|it| (it.author(), it)).collect()
        }
        let Gossip {
            decided,
            prepares,
            promises,
            proposals,
            votes,
        } = saved.gossip;

        Self {
            founding: saved.founding,
            decided,
            slot: Slot {
                promised: saved.promised,
                accepted: saved.accepted,
                candidate: saved.candidate,
                prepares: trusted(prepares),
                promises: trusted(promises),
                proposals: trusted(proposals),
                votes: trusted(votes),
            },
        }
    }

    /// Starts an attempt to make `body` the next checkpoint, with
    /// `electorate` deciding the one after. Only an elector can, and the
    /// attempt may end up carrying someone else's checkpoint instead.
    pub(crate) fn propose(
        &mut self,
        identity: &Identity,
        electorate: Electorate,
        body: Vec<u8>,
    ) -> bool {
        let me = identity.peer();
        if !self.tip().electorate.includes(me) || electorate.is_empty() {
            return false;
        }

        let ballot = Ballot {
            round: self.slot.highest_round() + 1,
            proposer: me,
        };
        let prepare = Prepare {
            slot: self.tip().digest(),
            ballot,
        };
        self.slot.candidate = Some((ballot, self.tip().extend(electorate, body)));
        self.slot
            .prepares
            .insert(me, Signed::sign(identity, prepare));

        true
    }

    /// What to send a peer whose tip is at `height` or above.
    pub(crate) fn gossip(&self, height: u64) -> Gossip {
        fn signed<T: Clone>(held: &BTreeMap<Peer, Verified<T>>) -> Vec<Signed<T>> {
            held.values().map(|it| it.signed().clone()).collect()
        }
        let behind = |link: &&Certificate| link.checkpoint.height > height;

        Gossip {
            decided: self.decided.iter().filter(behind).cloned().collect(),
            prepares: signed(&self.slot.prepares),
            promises: signed(&self.slot.promises),
            proposals: signed(&self.slot.proposals),
            votes: signed(&self.slot.votes),
        }
    }

    /// Takes on whatever a peer holds that's new and valid. Returns
    /// whether anything was.
    pub(crate) fn receive(&mut self, gossip: Gossip) -> bool {
        let mut grew = false;
        for link in gossip.decided {
            grew |= self.adopt(link);
        }

        let tip = self.tip().clone();
        let slot = &mut self.slot;
        let proposer = |it: &Ballot, author: Peer| it.proposer == author;
        grew |= keep(&mut slot.prepares, gossip.prepares, &tip, |it, author| {
            proposer(&it.ballot, author)
        });
        grew |= keep(&mut slot.promises, gossip.promises, &tip, |it, _| {
            it.accepted.iter().all(|(_, voted)| voted.extends(&tip))
        });
        grew |= keep(&mut slot.proposals, gossip.proposals, &tip, |it, author| {
            proposer(&it.ballot, author) && it.checkpoint.extends(&tip)
        });
        grew |= keep(&mut slot.votes, gossip.votes, &tip, |_, _| true);

        grew
    }

    /// Does everything this device can do with what it holds: promise,
    /// vote, propose, and decide. Returns whether it did anything.
    ///
    /// Whatever this signs must be durable before it's sent.
    ///
    /// It votes only for a checkpoint that's `valid`: one whose contents
    /// this device has checked for itself. Promising needs no such check.
    pub(crate) fn advance(
        &mut self,
        identity: &Identity,
        valid: impl Fn(&Checkpoint) -> bool,
    ) -> bool {
        let mut advanced = false;
        loop {
            let mut moved = self.promise(identity);
            moved |= self.vote(identity, &valid);
            moved |= self.put(identity);
            moved |= self.decide();
            if !moved {
                return advanced;
            }
            advanced = true;
        }
    }

    /// Takes the next link from a peer, if its proof holds.
    fn adopt(&mut self, link: Certificate) -> bool {
        if link.checkpoint.height != self.tip().height + 1 {
            return false;
        }
        if !link.proves(self.tip()) {
            tracing::warn!(
                height = link.checkpoint.height,
                "ignored a checkpoint without proof"
            );
            return false;
        }

        self.extend(link);

        true
    }

    /// The slot is decided, and nothing said about it matters anymore.
    fn extend(&mut self, link: Certificate) {
        tracing::debug!(height = link.checkpoint.height, "checkpoint decided");
        self.decided.push(link);
        self.slot = Slot::default();
    }

    /// Promises the highest ballot anyone is preparing.
    fn promise(&mut self, identity: &Identity) -> bool {
        let me = identity.peer();
        let prepares = self.slot.prepares.values();
        let Some(ballot) = prepares.map(|it| it.payload().ballot).max() else {
            return false;
        };
        if !self.tip().electorate.includes(me) || self.slot.promised >= Some(ballot) {
            return false;
        }

        let promise = Promise {
            slot: self.tip().digest(),
            ballot,
            accepted: self.slot.accepted.clone(),
        };
        self.slot.promised = Some(ballot);
        self.slot
            .promises
            .insert(me, Signed::sign(identity, promise));

        true
    }

    /// Votes for the highest proposal, unless it promised a later ballot
    /// or already voted for this one.
    fn vote(&mut self, identity: &Identity, valid: impl Fn(&Checkpoint) -> bool) -> bool {
        let me = identity.peer();
        let proposals = self.slot.proposals.values().map(Verified::payload);
        let Some(proposal) = proposals.max_by_key(|it| it.ballot).cloned() else {
            return false;
        };
        let ballot = proposal.ballot;
        let voted = self.slot.accepted.as_ref().map(|(ballot, _)| *ballot);
        if !self.tip().electorate.includes(me)
            || self.slot.promised > Some(ballot)
            || voted >= Some(ballot)
            || !valid(&proposal.checkpoint)
        {
            return false;
        }

        let vote = Vote {
            slot: proposal.slot,
            ballot,
            checkpoint: proposal.checkpoint.digest(),
        };
        self.slot.promised = Some(ballot);
        self.slot.accepted = Some((ballot, proposal.checkpoint));
        self.slot.votes.insert(me, Signed::sign(identity, vote));

        true
    }

    /// Names the checkpoint for this device's attempt, once a quorum has
    /// promised it: the latest any of them voted for, or else its own.
    fn put(&mut self, identity: &Identity) -> bool {
        let me = identity.peer();
        let Some((ballot, mine)) = &self.slot.candidate else {
            return false;
        };
        let proposed = self.slot.proposals.get(&me);
        if proposed.is_some_and(|it| it.payload().ballot >= *ballot) {
            return false;
        }

        let promises = self.slot.promises.iter();
        let promises: Vec<_> = promises
            .filter(|(_, it)| it.payload().ballot == *ballot)
            .collect();
        let quorum = promises.iter().map(|(peer, _)| **peer).collect();
        if !self.tip().electorate.has_quorum(&quorum) {
            return false;
        }

        let voted = promises
            .iter()
            .filter_map(|(_, it)| it.payload().accepted.as_ref());
        let inherited = voted.max_by_key(|(ballot, _)| *ballot);
        let proposal = Proposal {
            slot: self.tip().digest(),
            ballot: *ballot,
            checkpoint: inherited.map_or(mine, |(_, voted)| voted).clone(),
        };
        self.slot
            .proposals
            .insert(me, Signed::sign(identity, proposal));

        true
    }

    /// Extends the chain once a quorum has voted for one checkpoint in
    /// one ballot. Needs no say in the matter, only the votes.
    fn decide(&mut self) -> bool {
        let mut tally: BTreeMap<(Ballot, Digest), BTreeSet<Peer>> = BTreeMap::new();
        for (voter, vote) in &self.slot.votes {
            let vote = vote.payload();
            let voters = tally.entry((vote.ballot, vote.checkpoint)).or_default();
            voters.insert(*voter);
        }

        let electorate = &self.tip().electorate;
        let mut won = tally.iter().filter(|(_, by)| electorate.has_quorum(by));
        let Some(((ballot, digest), quorum)) = won.next() else {
            return false;
        };
        let Some(checkpoint) = self.slot.voted_on(*ballot, *digest) else {
            return false;
        };

        let votes = quorum.iter().filter_map(|voter| self.slot.votes.get(voter));
        let link = Certificate {
            checkpoint,
            ballot: *ballot,
            votes: votes.map(|vote| vote.signed().clone()).collect(),
        };
        self.extend(link);

        true
    }
}

impl Slot {
    /// The highest round anyone has mentioned, so a new attempt can
    /// outrank all of them.
    fn highest_round(&self) -> u64 {
        fn rounds<T: Stated>(held: &BTreeMap<Peer, Verified<T>>) -> Option<u64> {
            held.values().map(|it| it.payload().ballot().round).max()
        }
        let heard = [
            rounds(&self.prepares),
            rounds(&self.promises),
            rounds(&self.proposals),
            rounds(&self.votes),
            self.promised.map(|ballot| ballot.round),
        ];

        heard.into_iter().flatten().max().unwrap_or(0)
    }

    /// The checkpoint a vote was for, if this device has seen it.
    fn voted_on(&self, ballot: Ballot, digest: Digest) -> Option<Checkpoint> {
        let proposals = self.proposals.values().map(Verified::payload);
        let proposed = proposals
            .filter(|it| it.ballot == ballot)
            .map(|it| &it.checkpoint);
        let accepted = self.accepted.iter().map(|(_, voted)| voted);

        proposed
            .chain(accepted)
            .find(|it| it.digest() == digest)
            .cloned()
    }
}

/// Keeps each elector's latest statement about the slot after `tip`, if
/// it's `valid`. Returns whether anything was new.
fn keep<T: Stated + Signable>(
    held: &mut BTreeMap<Peer, Verified<T>>,
    heard: Vec<Signed<T>>,
    tip: &Checkpoint,
    valid: impl Fn(&T, Peer) -> bool,
) -> bool {
    let mut grew = false;
    for statement in verify_all(heard) {
        let (author, said) = (statement.author(), statement.payload());
        let current = said.slot() == tip.digest() && tip.electorate.includes(author);
        let newer = held
            .get(&author)
            .is_none_or(|it| it.payload().ballot() < said.ballot());
        if current && newer && valid(said, author) {
            held.insert(author, statement);
            grew = true;
        }
    }

    grew
}

#[cfg(test)]
mod tests;
