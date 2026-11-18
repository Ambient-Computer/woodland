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
#[cfg(feature = "blobs")]
mod blobs;
mod changed;
mod configuration;
pub(crate) mod entry;
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
pub(crate) mod step;
mod succession;
#[cfg(test)]
mod tests;
mod transaction;
mod veiled;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ed25519_dalek::VerifyingKey;

use crate::actor::{Identity, Peer};
use crate::blob::tally;
use crate::chain::{Chain, Checkpoint, Electorate, Role};
use crate::encoding;
use crate::model::Operation;
use crate::seal::Secret;
use crate::store::{Store, StoreError};

#[cfg(feature = "blobs")]
pub use crate::blob::want::BlobPolicy;
pub use answer::{Answer, Found, Part, Reached};
#[cfg(feature = "blobs")]
pub use blobs::{Import, ImportId};
pub use changed::Changed;
pub use configuration::{Configuration, Reconfigured};
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
    /// Whether the bytes of some blob are being deleted.
    sweeping: bool,
    #[cfg(feature = "blobs")]
    blobs: blobs::Blobs,
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
    /// What's become of what this device [asked] its groups to be
    /// made up of. Told once.
    ///
    /// [asked]: Database::reconfigure
    pub reconfigured: Vec<(GroupId, Reconfigured)>,
    /// Whether there's more to do that waits for nothing. A step does
    /// only so much, so that none takes long or writes a great deal
    /// at once. Call [`Database::resume`] until this is false.
    pub unfinished: bool,
    /// The peers there's something to say to about blobs: take it
    /// with [`Database::next_blob_message`]. Always empty without
    /// the `blobs` feature.
    ///
    /// [`Database::next_blob_message`]: Database#method.next_blob_message
    pub blobs: Vec<VerifyingKey>,
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
    /// The transaction refers to a blob with a reference made for the
    /// other kind of group. A sealed group's blobs are encrypted and
    /// an open group's aren't, so the bytes are imported again.
    BlobMisplaced,
    /// The transaction refers to a blob this device doesn't hold all
    /// of. A device writes a reference only to what it could hand
    /// over, even one it was sent.
    BlobNotHeld,
    /// Nothing this device holds refers to that blob, so there's no
    /// one to fetch it from and nothing to keep it for.
    BlobUnreferred,
    /// The device hasn't said which blobs it fetches. There's no
    /// default: say, and then hold the group.
    NoBlobPolicy,
    /// There's no such import: it was never begun, or it's finished
    /// or abandoned.
    UnknownImport,
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
            Self::BlobNotHeld => formatter.write_str("this device doesn't hold all of that blob"),
            Self::BlobUnreferred => formatter.write_str("nothing here refers to that blob"),
            Self::NoBlobPolicy => {
                formatter.write_str("the device hasn't said which blobs it fetches")
            }
            Self::UnknownImport => formatter.write_str("there's no such import"),
            Self::BlobMisplaced => {
                formatter.write_str("a blob reference made for the other kind of group")
            }
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
    /// So pass fresh random bytes every time, and ask for the
    /// [secret] the device has afterwards.
    ///
    /// [secret]: Self::device_secret
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

        let tallied = tally::tally(&mut step).await?;
        #[cfg(feature = "blobs")]
        let blobs = blobs::Blobs::load(&step).await?;
        #[cfg(feature = "blobs")]
        let sweeping = blobs::settle(&mut step, tallied).await?;
        #[cfg(not(feature = "blobs"))]
        let sweeping = {
            drop(tallied);

            false
        };
        store.apply(step.finish().batch).await?;
        let mut database = Self {
            store,
            identity,
            groups,
            peers: BTreeSet::new(),
            broken: false,
            sweeping,
            #[cfg(feature = "blobs")]
            blobs,
        };
        database.pin();

        Ok(database)
    }

    /// The key peers know this device by.
    pub fn id(&self) -> VerifyingKey {
        self.identity.public_key()
    }

    /// The device's secret: the one the store holds, whatever was
    /// passed to [`open`]. It's for the transport, which is the same
    /// peer and signs with the same key.
    ///
    /// Read it here each time the device starts, and keep it nowhere
    /// else. A copy that outlives the store would bring the device
    /// back as a peer that has forgotten what it signed, and one
    /// written into a group is handed to whoever holds the group.
    ///
    /// [`open`]: Self::open
    #[must_use]
    pub fn device_secret(&self) -> [u8; 32] {
        self.identity.secret_key()
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
        #[cfg(feature = "blobs")]
        if self.blobs.policy.is_none() {
            return Err(Error::NoBlobPolicy);
        }
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
        let clock = transaction.clock(group.into());
        let clock = clock.ok_or(Error::ManyClocks)?;
        let blobs = transaction.operations.iter().flat_map(Operation::blobs);
        #[cfg(feature = "blobs")]
        for blob in blobs {
            let whole = crate::blob::kept::is_whole(&step, blob.listed()).await;
            if !self.guard(whole)? {
                return Err(Error::BlobNotHeld);
            }
        }
        // A device that keeps no bytes holds no blob.
        #[cfg(not(feature = "blobs"))]
        if blobs.count() > 0 {
            return Err(Error::BlobNotHeld);
        }
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let written = held
            .write(&self.identity, clock, transaction.operations, &mut step)
            .await;
        match self.guard(written)? {
            Wrote::Yes(seq) => {
                let wrote = TransactionId {
                    group,
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
            Wrote::Misplaced => Err(Error::BlobMisplaced),
        }
    }

    /// Asks the group to give these peers these roles from the next
    /// checkpoint on. Anyone left out is removed; anyone new is added.
    /// It happens when a checkpoint carries it.
    ///
    /// It names everyone, so build it from the [configuration] in
    /// force and not from a list kept elsewhere. It's made of the
    /// group as this device knows it: if the group changes some other
    /// way first, the request is spent and does nothing, and
    /// [`Outputs::reconfigured`] says so. Read the configuration again
    /// and ask again, if what was wanted still is.
    ///
    /// [configuration]: Self::configuration
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
        #[cfg(feature = "blobs")]
        self.blobs.hunger();
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
        #[cfg(feature = "blobs")]
        self.blobs.disconnected(peer);
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

            #[cfg(feature = "blobs")]
            let shared = group.shares(from);
            let received = group
                .receive(&self.identity, from, message, &mut step)
                .await;
            // A peer that turns out to hold the group may have its blobs.
            #[cfg(feature = "blobs")]
            if !shared && group.shares(from) {
                self.blobs.hunger();
            }
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
        #[cfg(feature = "blobs")]
        self.blobs.tick();
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
        #[cfg(feature = "blobs")]
        self.sweep(&mut step).await?;

        self.finish(step).await
    }

    /// Whether there's more to do that waits for nothing.
    fn is_unfinished(&self) -> bool {
        #[cfg(feature = "blobs")]
        let looking = self.blobs.is_busy();
        #[cfg(not(feature = "blobs"))]
        let looking = false;

        looking || self.sweeping || self.groups.values().any(Group::is_busy)
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

        // Last of all, since it goes by everything the step wrote.
        let tallied = tally::tally(&mut step).await;
        let tallied = self.guard(tallied)?;
        #[cfg(feature = "blobs")]
        {
            let followed = self.follow(&mut step, tallied).await;
            self.guard(followed)?;
        }
        #[cfg(not(feature = "blobs"))]
        drop(tallied);

        let mut finished = step.finish();
        let hidden = |change: &Changed| self.groups.values().any(|group| group.hides(change));
        finished.changed.retain(|change| !hidden(change));
        // A step that wrote nothing has nothing to make durable.
        if !finished.batch.is_empty() {
            let applied = self.store.apply(finished.batch).await;
            self.guard(applied)?;
        }
        self.pin();
        let sends = finished.sends.into_iter();
        let me = self.identity.peer();
        let mine = |told: &step::Told| told.author == me;
        let named = |told: step::Told| {
            let id = TransactionId {
                group: told.group,
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
            reconfigured: finished.reconfigured,
            unfinished: self.is_unfinished(),
            #[cfg(feature = "blobs")]
            blobs: self.blobs.waiting(),
            #[cfg(not(feature = "blobs"))]
            blobs: Vec::new(),
        })
    }
}
