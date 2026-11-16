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

mod changed;
mod entry;
mod group;
mod id;
pub(crate) mod keys;
mod message;
mod step;
#[cfg(test)]
mod tests;
mod transaction;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ed25519_dalek::VerifyingKey;

use crate::actor::{Identity, Peer};
use crate::chain::{Checkpoint, Electorate, Role};
use crate::encoding;
use crate::model::Query;
use crate::store::{Store, StoreError};

pub use changed::Changed;
use group::Group;
pub use id::{ClockId, GroupId};
use message::Message;
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
}

#[derive(Debug)]
pub enum Error {
    /// The store failed. The database is unusable until it's reopened.
    Store(StoreError),
    /// An earlier step failed. Reopen the database from its store.
    Broken,
    /// This device doesn't hold that group.
    UnknownGroup,
    /// This device isn't one of the group's contributors.
    NotAContributor,
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
            Self::NotAContributor => formatter.write_str("this device isn't a contributor"),
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

        let identity = Identity::from_secret_key(stored.unwrap_or(secret));
        let mut groups = BTreeMap::new();
        let (start, end) = keys::range(keys::foundings());
        for (_, founding) in step.scan(&start, &end).await? {
            let Some(founding) = encoding::decode::<Checkpoint>(&founding) else {
                continue;
            };

            let id = group::id_of(&founding);
            if let Some(group) = Group::load(id, &step).await? {
                groups.insert(id, group);
            }
        }

        store.apply(step.finish().batch).await?;

        Ok(Self {
            store,
            identity,
            groups,
            peers: BTreeSet::new(),
            broken: false,
        })
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

        self.hold(founding).await
    }

    /// What another device needs to [join] a group this one holds.
    ///
    /// [join]: Self::join
    pub fn invite(&self, group: GroupId) -> Option<Vec<u8>> {
        let founding = self.groups.get(&group)?.founding()?;

        Some(encoding::encode(founding))
    }

    /// Starts holding a group another device [invited] this one to. It
    /// can read and relay at once. It can write when the group's
    /// contributors make it one.
    ///
    /// [invited]: Self::invite
    ///
    /// # Errors
    ///
    /// If the invitation is unreadable, or the store fails.
    pub async fn join(&mut self, invitation: &[u8]) -> Result<(GroupId, Outputs), Error> {
        let founding = encoding::decode::<Checkpoint>(invitation);
        let founding = founding.filter(|it| it.height == 0);

        self.hold(founding.ok_or(Error::BadInvitation)?).await
    }

    async fn hold(&mut self, founding: Checkpoint) -> Result<(GroupId, Outputs), Error> {
        let id = group::id_of(&founding);
        let mut step = self.begin()?;
        if !self.groups.contains_key(&id) {
            let mut group = Group::start(founding, &mut step);
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
        if !self.guard(written)? {
            return Err(Error::NotAContributor);
        }

        self.finish(step).await
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

    /// Sheds the history of `group` that's committed, keeping committed
    /// state and whatever isn't committed yet. Nothing reads differently
    /// afterwards. A peer that needs what was shed is sent the state
    /// instead.
    ///
    /// A device that never calls this is an archive: it can hand any
    /// peer the group's whole history.
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails.
    pub async fn compact(&mut self, group: GroupId) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let shed = held.shed(&self.identity, &mut step).await;
        self.guard(shed)?;

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
        let from = Peer::from(from);
        let mut step = self.begin()?;
        let Some(message) = encoding::decode::<Message>(message) else {
            tracing::warn!(peer = ?from, "dropped an unreadable message");
            return Ok(Outputs::default());
        };
        let Some(group) = self.groups.get_mut(&message.group) else {
            return Ok(Outputs::default());
        };

        let received = group
            .receive(&self.identity, from, message, &mut step)
            .await;
        self.guard(received)?;

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

    /// Answers `query` from whole steps: all of a step's writes, or none.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn read(&self, query: &Query) -> Result<Vec<(Vec<u8>, Vec<u8>)>, Error> {
        let view = self.store.view();
        let mut scanned = Vec::new();
        for (start, end) in query.storage_ranges() {
            scanned.extend(step::scan(&view, &start, &end).await?);
        }

        Ok(query.results(&scanned))
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

    async fn finish(&mut self, step: Step<S::View>) -> Result<Outputs, Error> {
        let finished = step.finish();
        let applied = self.store.apply(finished.batch).await;
        self.guard(applied)?;
        let sends = finished.sends.into_iter();

        Ok(Outputs {
            sends: sends.map(|(peer, message)| (peer.id(), message)).collect(),
            changed: finished.changed,
        })
    }
}
