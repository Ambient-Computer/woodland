//! The database: every group one device holds, over one store.
//!
//! The application hands it what happens (a local write, a message from
//! a peer, the passing of time) and gets back what to do about it:
//! messages to deliver, and notices of what changed. It opens no
//! connections and reads no clock.
//!
//! Each call is one step. Everything the step changes reaches the store
//! as one batch, and what it wants said is handed back only after that
//! batch is durable, so a device never says something it could forget.

mod answer;
mod changed;
mod entry;
mod group;
mod holding;
mod id;
pub(crate) mod keys;
mod message;
mod outcome;
mod parts;
mod pending;
mod proof;
mod reading;
mod sealed;
mod step;
mod succession;
#[cfg(test)]
mod tests;
mod transaction;
mod veiled;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ed25519_dalek::VerifyingKey;

use crate::actor::{Identity, Peer};
use crate::chain::{Chain, Checkpoint, Electorate, Role};
use crate::encoding;
use crate::seal::Secret;
use crate::store::{Store, StoreError};

pub use answer::{Answer, Found, Part, Reached};
pub use changed::Changed;
use group::{Group, Invitation, Wrote};
pub use id::{ClockId, GroupId};
use message::Message;
pub use outcome::{Outcome, TransactionId};
pub use pending::{Pending, Waiting};
use step::Step;
pub use transaction::Transaction;

/// One device's replica of every group it holds.
#[derive(Debug)]
pub struct Database<S> {
    store: S,
    identity: Identity,
    groups: BTreeMap<GroupId, Group>,
    peers: BTreeSet<Peer>,
    /// Set when a step failed partway. What's in memory can no longer be
    /// trusted to match the store.
    broken: bool,
}

/// What a step asks of the application, once the step is durable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[must_use = "messages have to be delivered"]
pub struct Outputs {
    /// Messages, and the peer each one is for.
    pub sends: Vec<(VerifyingKey, Vec<u8>)>,
    /// What may read differently now.
    pub changed: Vec<Changed>,
    /// The transaction this step wrote, if it was a write.
    pub wrote: Option<TransactionId>,
    /// What's become of transactions this device wrote, in the order
    /// it happened. Told once: [`Database::outcome`] says the last
    /// of it again.
    pub outcomes: Vec<(TransactionId, Outcome)>,
    /// Whether there's more to do that waits for nothing. A step does
    /// only so much, so that none takes long or writes a great deal
    /// at once. Call [`Database::resume`] until this is false.
    pub unfinished: bool,
}

#[derive(Debug)]
pub enum Error {
    /// The store failed. The database is unusable until it's reopened.
    Store(StoreError),
    /// An earlier step failed. Reopen the database from its store.
    Broken,
    /// This device doesn't hold that group.
    UnknownGroup,
    /// The device has a say in the group, and has to hold all of it.
    Votes,
    /// The device has let checkpoints go by without a clock it now
    /// wants. It has to [forget] the group and join again.
    ///
    /// [forget]: Database::forget
    PassedOver,
    /// The device has a role in the group, and can't [forget] it.
    ///
    /// [forget]: Database::forget
    StillAMember,
    /// This device isn't one of the group's contributors.
    NotAContributor,
    /// This device can't read what it was asked to write to: it's
    /// sealed and the device lacks the secret, or it holds something
    /// written by a version of the library that's newer than this one.
    Unreadable,
    /// The invitation couldn't be read.
    BadInvitation,
    /// The transaction writes to models of more than one clock.
    ManyClocks,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Broken => formatter.write_str("an earlier step failed, reopen the database"),
            Self::UnknownGroup => formatter.write_str("this device doesn't hold that group"),
            Self::Votes => formatter.write_str("this device has a say, and holds every clock"),
            Self::PassedOver => {
                formatter.write_str("this device has passed over a clock it now wants")
            }
            Self::StillAMember => {
                formatter.write_str("this device is still a member of that group")
            }
            Self::NotAContributor => formatter.write_str("this device isn't a contributor"),
            Self::Unreadable => formatter.write_str("this device can't read that"),
            Self::BadInvitation => formatter.write_str("the invitation couldn't be read"),
            Self::ManyClocks => formatter.write_str("a transaction writes to one clock's models"),
        }
    }
}

impl std::error::Error for Error {}

impl From<StoreError> for Error {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl<S: Store> Database<S> {
    /// Opens the database `store` holds.
    ///
    /// A device's key lives in its store, beside everything it signed. A
    /// store that already has one keeps it, and `secret` is used only by
    /// a store that's new: a device that lost its store comes back as
    /// someone else.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn open(mut store: S, secret: [u8; 32]) -> Result<Self, Error> {
        let mut step = Step::new(store.view());
        let stored = step.load::<[u8; 32]>(&keys::identity()).await?;
        if stored.is_none() {
            step.save(keys::identity(), &secret);
        }

        let mut identity = Identity::from_secret_key(stored.unwrap_or(secret));
        let renewals = step.load::<Vec<[u8; 32]>>(&keys::sealing()).await?;
        for fresh in renewals.unwrap_or_default() {
            identity.renew(fresh);
        }
        let mut groups = BTreeMap::new();
        let (start, end) = keys::range(keys::foundings());
        for (_, founding) in step.scan(&start, &end).await? {
            let Some(founding) = encoding::decode::<Checkpoint>(&founding) else {
                continue;
            };

            let id = group::id_of(&founding);
            if let Some(mut group) = Group::load(id, &step).await? {
                group.outgrow(&mut step).await?;
                groups.insert(id, group);
            }
        }

        store.apply(step.finish().batch).await?;
        let mut database = Self {
            store,
            identity,
            groups,
            peers: BTreeSet::new(),
            broken: false,
        };
        database.pin();

        Ok(database)
    }

    /// The key peers know this device by.
    pub fn id(&self) -> VerifyingKey {
        self.identity.public_key()
    }

    /// The groups this device holds.
    pub fn groups(&self) -> impl Iterator<Item = GroupId> + '_ {
        self.groups.keys().copied()
    }

    /// Starts a group with this device as its only contributor. `seed`
    /// is what no other group shares: random bytes.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn found(&mut self, seed: [u8; 32]) -> Result<(GroupId, Outputs), Error> {
        let me = self.identity.peer();
        let founding = Checkpoint::founding(seed, Electorate::new([(me, 1)]));

        self.hold(Chain::new(founding), None).await
    }

    /// What another device needs to [join] a group this one holds:
    /// where the group began, and proof of how far it has got. Give it
    /// only to a device that should trust this one about that.
    ///
    /// [join]: Self::join
    pub fn invite(&self, group: GroupId) -> Option<Vec<u8>> {
        let invitation = self.groups.get(&group)?.invitation()?;

        Some(encoding::encode(&invitation))
    }

    /// Starts holding a group another device [invited] this one to. It
    /// can read and relay at once. It can write when the group's
    /// contributors make it one.
    ///
    /// The group is taken to be at least as far along as the inviter
    /// said. Nothing that contradicts that is believed afterwards, so
    /// an invitation is only as good as whoever gave it.
    ///
    /// [invited]: Self::invite
    ///
    /// # Errors
    ///
    /// If the invitation is unreadable or doesn't prove what it claims,
    /// or the store fails.
    pub async fn join(&mut self, invitation: &[u8]) -> Result<(GroupId, Outputs), Error> {
        let invitation = encoding::decode::<Invitation>(invitation);
        let chain = invitation.and_then(Invitation::chain);

        self.hold(chain.ok_or(Error::BadInvitation)?, None).await
    }

    async fn hold(
        &mut self,
        chain: Chain,
        secret: Option<Secret>,
    ) -> Result<(GroupId, Outputs), Error> {
        let founding = chain.checkpoints().next().ok_or(Error::BadInvitation)?;
        let id = group::id_of(founding);
        let mut step = self.begin()?;
        if !self.groups.contains_key(&id) {
            let mut group = Group::start(chain, &mut step).ok_or(Error::BadInvitation)?;
            let unlocked = group.unlock(&self.identity, &mut step).await;
            self.guard(unlocked)?;
            if let Some(secret) = secret {
                group.hold(&self.identity, 0, secret, &mut step);
            }
            let peers: Vec<Peer> = self.peers.iter().copied().collect();
            for peer in peers {
                let greeted = group.connected(peer, &mut step).await;
                self.guard(greeted)?;
            }
            self.groups.insert(id, group);
        }

        Ok((id, self.finish(step).await?))
    }

    /// Makes `transaction` part of the group's history. It's readable
    /// here at once, and committed when a checkpoint covers it.
    ///
    /// # Errors
    ///
    /// If this device may not write to the transaction's clock, the
    /// transaction writes to more than one, or the store fails.
    pub async fn write(
        &mut self,
        group: GroupId,
        transaction: Transaction,
    ) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let clock = transaction.clock(group.into());
        let clock = clock.ok_or(Error::ManyClocks)?;
        let written = held
            .write(&self.identity, clock, transaction.operations, &mut step)
            .await;
        match self.guard(written)? {
            Wrote::Yes(seq) => {
                let wrote = TransactionId {
                    clock,
                    author: self.identity.peer().id(),
                    seq,
                };
                let outputs = self.finish(step).await?;

                Ok(Outputs {
                    wrote: Some(wrote),
                    ..outputs
                })
            }
            Wrote::NotAllowed => Err(Error::NotAContributor),
            Wrote::Unreadable => Err(Error::Unreadable),
        }
    }

    /// Asks the group to give these peers these roles from the next
    /// checkpoint on. Anyone left out is removed; anyone new is added.
    /// It happens when a checkpoint carries it.
    ///
    /// # Errors
    ///
    /// If this device has no say in `group`, nobody would be left with
    /// one, or the store fails.
    pub async fn reconfigure(
        &mut self,
        group: GroupId,
        roles: impl IntoIterator<Item = (VerifyingKey, Role)>,
    ) -> Result<Outputs, Error> {
        let roles = roles.into_iter();
        let electorate = Electorate::of(roles.map(|(peer, role)| (Peer::from(peer), role)));
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let asked = held
            .reconfigure(&self.identity, electorate, &mut step)
            .await;
        if !self.guard(asked)? {
            return Err(Error::NotAContributor);
        }

        self.finish(step).await
    }

    /// A peer can be reached.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn connected(&mut self, peer: VerifyingKey) -> Result<Outputs, Error> {
        let peer = Peer::from(peer);
        let mut step = self.begin()?;
        self.peers.insert(peer);
        for group in self.groups.values_mut() {
            let greeted = group.connected(peer, &mut step).await;
            if greeted.is_err() {
                self.broken = true;
            }
            greeted?;
        }

        self.finish(step).await
    }

    /// A peer can't be reached any more.
    pub fn disconnected(&mut self, peer: VerifyingKey) {
        let peer = Peer::from(peer);
        self.peers.remove(&peer);
        for group in self.groups.values_mut() {
            group.disconnected(peer);
        }
    }

    /// A message arrived from a peer. One that makes no sense is dropped.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn receive(&mut self, from: VerifyingKey, message: &[u8]) -> Result<Outputs, Error> {
        self.receive_all([(from, message)]).await
    }

    /// Several messages arrived, from one peer or many. They're taken
    /// in order, in one step: everything they change is stored
    /// together, and what they lead this device to say is said once
    /// that's done. It's the same as receiving each in turn, with one
    /// write to the store where there would have been one each.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn receive_all<'a>(
        &mut self,
        messages: impl IntoIterator<Item = (VerifyingKey, &'a [u8])>,
    ) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        for (from, message) in messages {
            let from = Peer::from(from);
            let Some(message) = encoding::decode::<Message>(message) else {
                tracing::warn!(peer = ?from, "dropped an unreadable message");
                continue;
            };
            let Some(group) = self.groups.get_mut(&message.group) else {
                continue;
            };

            let received = group
                .receive(&self.identity, from, message, &mut step)
                .await;
            self.guard(received)?;
        }

        self.finish(step).await
    }

    /// Time has passed. Call it now and then: it's when checkpoints are
    /// attempted, and when a stalled one is retried.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn tick(&mut self) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        for group in self.groups.values_mut() {
            let ticked = group.tick(&self.identity, &mut step).await;
            if ticked.is_err() {
                self.broken = true;
            }
            ticked?;
        }

        self.finish(step).await
    }

    /// Carries on with whatever earlier steps left unfinished: see
    /// [`Outputs::unfinished`].
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn resume(&mut self) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        self.inherit(&mut step).await?;
        for group in self.groups.values_mut().filter(|group| group.is_busy()) {
            let resumed = group.resume(&self.identity, &mut step).await;
            if resumed.is_err() {
                self.broken = true;
            }
            resumed?;
        }

        self.finish(step).await
    }

    fn begin(&self) -> Result<Step<S::View>, Error> {
        if self.broken {
            return Err(Error::Broken);
        }

        Ok(Step::new(self.store.view()))
    }

    /// A step that fails partway leaves memory ahead of the store.
    fn guard<T>(&mut self, result: Result<T, StoreError>) -> Result<T, Error> {
        self.broken |= result.is_err();

        Ok(result?)
    }

    async fn finish(&mut self, mut step: Step<S::View>) -> Result<Outputs, Error> {
        for group in self.groups.values_mut() {
            let raised = group.raise(&mut step).await;
            if raised.is_err() {
                self.broken = true;
            }
            raised?;
        }

        let changes = step.changes();
        for group in self.groups.values_mut() {
            let derived = group.derive(&changes, &mut step).await;
            if derived.is_err() {
                self.broken = true;
            }
            derived?;
        }

        for group in self.groups.values_mut() {
            let veiled = group.veil(&mut step).await;
            if veiled.is_err() {
                self.broken = true;
            }
            veiled?;
        }

        // What became of this device's own transactions is kept, so
        // an application that wasn't there to be told can ask.
        let me = self.identity.peer();
        let mine = step.told().iter().filter(|told| told.author == me);
        let kept: Vec<_> = mine
            .map(|told| {
                let key = keys::outcome(told.group, told.clock, told.author, told.seq);

                (key, vec![told.outcome.to_byte()])
            })
            .collect();
        for (key, outcome) in kept {
            step.put(key, outcome);
        }

        let mut finished = step.finish();
        let hidden = |change: &Changed| self.groups.values().any(|group| group.hides(change));
        finished.changed.retain(|change| !hidden(change));
        let applied = self.store.apply(finished.batch).await;
        self.guard(applied)?;
        self.pin();
        let sends = finished.sends.into_iter();
        let me = self.identity.peer();
        let mine = |told: &step::Told| told.author == me;
        let named = |told: step::Told| {
            let id = TransactionId {
                clock: told.clock,
                author: told.author.id(),
                seq: told.seq,
            };

            (id, told.outcome)
        };

        Ok(Outputs {
            sends: sends.map(|(peer, message)| (peer.id(), message)).collect(),
            changed: finished.changed,
            wrote: None,
            outcomes: finished
                .outcomes
                .into_iter()
                .filter(mine)
                .map(named)
                .collect(),
            unfinished: self.groups.values().any(Group::is_busy),
        })
    }
}
