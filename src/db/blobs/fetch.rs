//! Fetching blobs from connected peers, and serving them.
//!
//! Blob traffic is apart from everything else a device says. What
//! this device wants to ask, and what it's been asked, wait here
//! until the application takes the next message for a peer: so a
//! blob is never ahead of anything in the queue the application
//! didn't put it ahead of.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ed25519_dalek::VerifyingKey;

use super::super::group::Group;
use super::super::{Changed, Database, Error, GroupId, Outputs, Step, keys as held};
use crate::BlobRef;
use crate::actor::Peer;
use crate::blob::fetch::{self, Accepted, Message, Part};
use crate::blob::keys::{self, Place};
use crate::blob::tally::{self, Tallied};
use crate::blob::want::{self, BlobPolicy};
use crate::blob::{Listed, kept};
use crate::encoding;
use crate::model::ModelId;
use crate::store::{Store, StoreError, View};

/// How many parts are asked of one peer before any has come back,
/// once it has shown it has the blob. Until then it's asked for one.
const WINDOW: usize = 4;

/// How many answers one peer may be owed. What it asks past that is
/// dropped, and it asks again.
const OWED: usize = 16;

/// How many of the blobs the application asked for are looked at in
/// one step, and how many of the places a blob may be wanted from.
const ASKED: usize = 16;
const WANTED: usize = 64;

/// The most ticks a peer that lacked a blob is left alone for.
const PATIENCE: u32 = 64;

/// Something to say to a peer about blobs.
#[derive(Debug)]
enum Out {
    Ask(Message),
    /// What it asked for, read when it's taken.
    Answer {
        group: GroupId,
        blob: Listed,
        part: Part,
    },
}

/// A peer that lacked a blob, and how long it's left alone.
#[derive(Clone, Copy, Debug)]
struct Lack {
    /// How many ticks it was left for this time.
    wait: u32,
    /// How many of them are left.
    left: u32,
}

/// What a device keeps in mind about its blobs. None of it outlives
/// the connections it's about.
#[derive(Debug, Default)]
pub(in crate::db) struct Blobs {
    pub(in crate::db) policy: Option<BlobPolicy>,
    outbox: BTreeMap<Peer, VecDeque<Out>>,
    /// The parts that have been asked for, and of whom.
    flying: BTreeMap<(Listed, Part), Peer>,
    lacking: BTreeMap<(Peer, Listed), Lack>,
    /// The peers that have sent part of a blob.
    proven: BTreeSet<(Peer, Listed)>,
    /// Whether there may be something to ask for that hasn't been.
    hungry: bool,
    /// Where looking through what's wanted has got to, if it's
    /// partway: among what was asked for, and among the rest.
    looking: Option<(Vec<u8>, Vec<u8>)>,
}

impl Blobs {
    pub(in crate::db) async fn load<V: View>(step: &Step<V>) -> Result<Self, StoreError> {
        Ok(Self {
            policy: step.load(&keys::policy()).await?,
            ..Self::default()
        })
    }

    /// The peers there's something to say to.
    pub(in crate::db) fn waiting(&self) -> Vec<VerifyingKey> {
        let waiting = self.outbox.iter().filter(|(_, out)| !out.is_empty());

        waiting.map(|(peer, _)| peer.id()).collect()
    }

    /// Whether there's more to look through that waits for nothing.
    pub(in crate::db) fn is_busy(&self) -> bool {
        self.hungry && self.looking.is_some()
    }

    pub(in crate::db) fn disconnected(&mut self, peer: Peer) {
        self.outbox.remove(&peer);
        self.flying.retain(|_, asked| *asked != peer);
        self.lacking.retain(|(lacked, _), _| *lacked != peer);
        self.proven.retain(|(proved, _)| *proved != peer);
    }

    /// Time has passed. What was asked for and never came is asked
    /// for again, and a peer that lacked a blob is nearer to being
    /// asked again.
    pub(in crate::db) fn tick(&mut self) {
        self.flying.clear();
        for lack in self.lacking.values_mut() {
            lack.left = lack.left.saturating_sub(1);
        }
        self.hungry = true;
    }

    pub(in crate::db) fn hunger(&mut self) {
        self.hungry = true;
    }

    fn lacks(&mut self, peer: Peer, blob: Listed) {
        let wait = self
            .lacking
            .get(&(peer, blob))
            .map_or(1, |lack| lack.wait * 2);
        let wait = wait.min(PATIENCE);
        self.lacking.insert((peer, blob), Lack { wait, left: wait });
        self.proven.remove(&(peer, blob));
    }

    /// How many more parts of `blob` may be asked of `peer` now.
    fn room(&self, peer: Peer, blob: Listed) -> usize {
        let lack = self.lacking.get(&(peer, blob));
        let asked = self.flying.values().filter(|of| **of == peer).count();
        let proven = self.proven.contains(&(peer, blob));
        let most = if proven { WINDOW } else { 1 };
        if lack.is_some_and(|lack| lack.left > 0) {
            return 0;
        }

        most.saturating_sub(asked)
    }
}

/// Follows what a step did to which blobs are referred to: bytes
/// nothing refers to start to go, and bytes that were going stay.
/// Says whether anything is left to delete.
pub(in crate::db) async fn settle<V: View>(
    step: &mut Step<V>,
    tallied: Tallied,
) -> Result<bool, StoreError> {
    for blob in tallied.freed {
        kept::free(step, blob).await?;
        want::settle(step, blob).await?;
        step.delete(keys::released(blob));
    }
    for blob in tallied.revived {
        kept::revive(step, blob).await?;
    }

    kept::is_sweeping(step).await
}

impl<S: Store> Database<S> {
    /// Says which blobs this device fetches without being asked. It
    /// has to be said before the device holds any group, and may be
    /// said again at any time. It's kept in the store: a device that
    /// starts again goes by what it last said.
    ///
    /// It covers the blobs a read would find a reference to. One the
    /// policy leaves out is fetched when the application [asks].
    ///
    /// [asks]: Self::fetch_blob
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn set_blob_policy(&mut self, policy: BlobPolicy) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        step.save(keys::policy(), &policy);
        self.blobs.policy = Some(policy);
        self.blobs.hunger();

        self.finish(step).await
    }

    /// Which blobs this device fetches without being asked, if it has
    /// said.
    #[must_use]
    pub fn blob_policy(&self) -> Option<BlobPolicy> {
        self.blobs.policy
    }

    /// Says which of one model's blobs this device fetches without
    /// being asked, where that isn't what it [said] for itself.
    /// `None` takes that back.
    ///
    /// References whose blobs are wanted differently belong in
    /// different models.
    ///
    /// [said]: Self::set_blob_policy
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn set_model_blob_policy(
        &mut self,
        model: ModelId,
        policy: Option<BlobPolicy>,
    ) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        match policy {
            Some(policy) => step.save(keys::policy_of(model), &policy),
            None => step.delete(keys::policy_of(model)),
        }
        self.blobs.hunger();

        self.finish(step).await
    }

    /// Asks for a blob, whatever the policy says. It's fetched from
    /// whichever connected peers have it, and asked for again as
    /// peers connect and time passes, until it's all here.
    ///
    /// # Errors
    ///
    /// If nothing this device holds refers to the blob, or the store
    /// fails.
    pub async fn fetch_blob(&mut self, blob: &BlobRef) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let blob = blob.listed();
        let referred = tally::is_referred(&step, blob).await;
        if !self.guard(referred)? {
            return Err(Error::BlobUnreferred);
        }

        let whole = kept::is_whole(&step, blob).await;
        if !self.guard(whole)? {
            step.put(keys::asked(blob), Vec::new());
            step.delete(keys::released(blob));
            self.blobs.hunger();
        }

        self.finish(step).await
    }

    /// Lets go of a blob's bytes. Whatever refers to it still does,
    /// and it's fetched again only when the application [asks].
    ///
    /// The bytes are deleted across steps: see
    /// [`Outputs::unfinished`].
    ///
    /// [asks]: Self::fetch_blob
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn release_blob(&mut self, blob: &BlobRef) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let blob = blob.listed();
        let settled = want::settle(&mut step, blob).await;
        self.guard(settled)?;
        let released = kept::release(&mut step, blob).await;
        self.guard(released)?;
        let referred = tally::is_referred(&step, blob).await;
        if self.guard(referred)? {
            step.put(keys::released(blob), Vec::new());
        }
        self.blobs.flying.retain(|(flying, _), _| *flying != blob);
        step.changed(Changed::Blob {
            name: blob.name,
            length: blob.length,
        });

        self.finish(step).await
    }

    /// The next thing to say to `peer` about blobs, if there's
    /// anything: [`Outputs::blobs`] says when there is. It's at most
    /// one part of one blob, read as it's taken.
    ///
    /// Send it apart from [`Outputs::sends`], on a stream of its own,
    /// and take the next when that one has gone. Blobs then never
    /// hold up anything else.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn next_blob_message(
        &mut self,
        peer: VerifyingKey,
    ) -> Result<Option<Vec<u8>>, Error> {
        let peer = Peer::from(peer);
        let next = self
            .blobs
            .outbox
            .get_mut(&peer)
            .and_then(VecDeque::pop_front);
        let (group, blob, part) = match next {
            None => return Ok(None),
            Some(Out::Ask(ask)) => return Ok(Some(encoding::encode(&ask))),
            Some(Out::Answer { group, blob, part }) => (group, blob, part),
        };

        // Knowing a name isn't enough: the blob is one that a group
        // both sides hold refers to.
        let step = Step::new(self.store.view());
        let shared = self.groups.get(&group).is_some_and(|it| it.shares(peer));
        let referred = step.get(&keys::refs(group, blob)).await?.is_some();
        let found = if shared && referred {
            fetch::serve(&step, blob, part).await?
        } else {
            None
        };
        let answer = match found {
            Some(bytes) => Message::Have { blob, part, bytes },
            None => Message::Lacks { blob, part },
        };

        Ok(Some(encoding::encode(&answer)))
    }

    /// Something a peer said about blobs arrived: what its
    /// [`next_blob_message`] gave.
    ///
    /// [`next_blob_message`]: Self::next_blob_message
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn receive_blob(
        &mut self,
        from: VerifyingKey,
        message: &[u8],
    ) -> Result<Outputs, Error> {
        let from = Peer::from(from);
        let mut step = self.begin()?;
        let Some(message) = encoding::decode::<Message>(message) else {
            tracing::warn!(peer = ?from, "dropped an unreadable message about blobs");

            return self.finish(step).await;
        };

        match message {
            Message::Ask { group, blob, part } => {
                let owed = self.blobs.outbox.entry(from).or_default();
                let answers = owed.iter().filter(|out| matches!(out, Out::Answer { .. }));
                if self.peers.contains(&from) && answers.count() < OWED {
                    owed.push_back(Out::Answer { group, blob, part });
                }
            }
            Message::Lacks { blob, part } => {
                if self.blobs.flying.get(&(blob, part)) == Some(&from) {
                    self.blobs.flying.remove(&(blob, part));
                    self.blobs.lacks(from, blob);
                    self.blobs.hunger();
                }
            }
            Message::Have { blob, part, bytes } => {
                // Only what was asked for is taken.
                if self.blobs.flying.get(&(blob, part)) != Some(&from) {
                    return self.finish(step).await;
                }

                self.blobs.flying.remove(&(blob, part));
                self.blobs.hunger();
                let released = step.get(&keys::released(blob)).await;
                let accepted = match self.guard(released)? {
                    Some(_) => Ok(Accepted::Dropped),
                    None => fetch::accept(&mut step, blob, part, bytes).await,
                };
                match self.guard(accepted)? {
                    Accepted::Dropped => self.blobs.lacks(from, blob),
                    Accepted::Stored | Accepted::Whole if !matches!(part, Part::Piece(_)) => {}
                    accepted => {
                        if accepted == Accepted::Whole {
                            let settled = want::settle(&mut step, blob).await;
                            self.guard(settled)?;
                        }
                        self.blobs.lacking.remove(&(from, blob));
                        self.blobs.proven.insert((from, blob));
                        step.changed(Changed::Blob {
                            name: blob.name,
                            length: blob.length,
                        });
                    }
                }
            }
        }

        self.finish(step).await
    }

    /// The last of a step, as far as blobs go: follows what it did to
    /// what's referred to, notes what it made wanted, and asks for
    /// what can be asked for.
    pub(in crate::db) async fn follow(
        &mut self,
        step: &mut Step<S::View>,
        tallied: Tallied,
    ) -> Result<(), StoreError> {
        self.sweeping = settle(step, tallied).await?;
        let groups = &self.groups;
        let reads = |group: GroupId| groups.get(&group).is_some_and(Group::reads);
        if want::note(step, reads).await? {
            self.blobs.hunger();
        }

        self.look(step).await
    }

    /// The first peer that could be asked for `blob`, and the group
    /// it's asked within.
    fn source(&self, blob: Listed, groups: &BTreeSet<GroupId>) -> Option<(Peer, GroupId)> {
        let shared = |peer: Peer| {
            let shares =
                |group: &&GroupId| self.groups.get(group).is_some_and(|it| it.shares(peer));

            groups.iter().find(shares).map(|group| (peer, *group))
        };
        let peers = self.peers.iter().copied();

        peers
            .filter(|peer| self.blobs.room(*peer, blob) > 0)
            .find_map(shared)
    }

    /// Asks every peer that can be asked for the parts of `blob` that
    /// are missing, a few each.
    async fn ask_for(
        &mut self,
        step: &Step<S::View>,
        blob: Listed,
        groups: &BTreeSet<GroupId>,
    ) -> Result<(), StoreError> {
        while let Some((peer, group)) = self.source(blob, groups) {
            let room = self.blobs.room(peer, blob);
            let flying = |part| self.blobs.flying.contains_key(&(blob, part));
            let parts = fetch::missing(step, blob, room, flying).await?;
            if parts.is_empty() {
                break;
            }

            for part in parts {
                self.blobs.flying.insert((blob, part), peer);
                let ask = Message::Ask { group, blob, part };
                self.blobs
                    .outbox
                    .entry(peer)
                    .or_default()
                    .push_back(Out::Ask(ask));
            }
        }

        Ok(())
    }

    /// Looks through a step's worth of what's wanted, and asks for
    /// what the policy covers and a connected peer may have.
    async fn look(&mut self, step: &mut Step<S::View>) -> Result<(), StoreError> {
        let Some(policy) = self.blobs.policy else {
            return Ok(());
        };
        if !self.blobs.hungry || self.peers.is_empty() {
            return Ok(());
        }

        let (asked_from, wanted_from) = self.blobs.looking.take().unwrap_or_default();
        let (start, end) = held::range(keys::all_asked());
        let start = start.max(asked_from);
        let asked = step.scan_page(&start, &end, ASKED).await?;
        let asked_next = asked.last().filter(|_| asked.len() == ASKED);
        let asked_next = asked_next.map(|(key, _)| held::after(key));
        for (key, _) in &asked {
            let Some(blob) = keys::blob_of(key) else {
                continue;
            };
            let (start, end) = held::range(keys::referrers(blob));
            let referring = step.scan(&start, &end).await?;
            let group = |(key, _): &(Vec<u8>, Vec<u8>)| key.last_chunk::<32>().copied();
            let groups = referring.iter().filter_map(group);
            let groups = groups.map(GroupId::from_bytes).collect();
            self.ask_for(step, blob, &groups).await?;
        }

        let (start, end) = held::range(keys::wants());
        let start = start.max(wanted_from);
        let wanted = step.scan_page(&start, &end, WANTED).await?;
        let wanted_next = wanted.last().filter(|_| wanted.len() == WANTED);
        let wanted_next = wanted_next.map(|(key, _)| held::after(key));
        let mut found: BTreeMap<Listed, BTreeSet<GroupId>> = BTreeMap::new();
        for (key, _) in wanted {
            let Some((blob, group, place)) = keys::want(&key) else {
                step.delete(key);
                continue;
            };
            let covers = match &place {
                Place::Listed => policy,
                Place::At { model, .. } => want::policy_of(step, *model).await?.unwrap_or(policy),
            };
            let groups = BTreeSet::from([group]);
            if !covers.covers(blob) || self.source(blob, &groups).is_none() {
                continue;
            }

            let reads = self.groups.get(&group).is_some_and(Group::reads);
            if want::stands(step, blob, group, &place, reads).await? {
                found.entry(blob).or_default().insert(group);
            } else {
                step.delete(key);
            }
        }
        for (blob, groups) in found {
            self.ask_for(step, blob, &groups).await?;
        }

        // Once it's all been looked through there's nothing more to
        // ask until something changes.
        self.blobs.looking = match (asked_next, wanted_next) {
            (None, None) => None,
            (asked, wanted) => {
                let past = |range: Vec<u8>| held::range(range).1;

                Some((
                    asked.unwrap_or_else(|| past(keys::all_asked())),
                    wanted.unwrap_or_else(|| past(keys::wants())),
                ))
            }
        };
        self.blobs.hungry = self.blobs.looking.is_some();

        Ok(())
    }
}
