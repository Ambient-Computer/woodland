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
mod outcome;
mod pending;
mod proof;
mod sealed;
mod step;
mod succession;
#[cfg(test)]
mod tests;
mod transaction;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ed25519_dalek::VerifyingKey;

use crate::actor::{Identity, Peer};
use crate::chain::{Chain, Checkpoint, Electorate, Role};
use crate::encoding;
use crate::model::{ColumnId, ModelId, Query};
use crate::seal::Secret;
use crate::store::{Store, StoreError};

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
    /// it happened. Told once.
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

    /// Starts keeping an index of `column` of `table`, so that the
    /// rows that hold a value in it can be found with [`Query::index`]
    /// without reading the table.
    ///
    /// An index is this device's own. Nothing is written to the group,
    /// no other device hears of it, and it changes nothing about how
    /// the column's cells are written or merged. It's filled a part at
    /// a time: [resume] until [`has_index`] says it has every row.
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails.
    ///
    /// [resume]: Self::resume
    /// [`has_index`]: Self::has_index
    pub async fn add_index(
        &mut self,
        group: GroupId,
        table: ModelId,
        column: ColumnId,
    ) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        held.index(table, column, &mut step);

        self.finish(step).await
    }

    /// Stops keeping an index of `column` of `table`, and throws away
    /// what it held.
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails.
    pub async fn drop_index(
        &mut self,
        group: GroupId,
        table: ModelId,
        column: ColumnId,
    ) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        held.unindex(table, column, &mut step);

        self.finish(step).await
    }

    /// Whether this device keeps an index of `column` of `table` that
    /// has every row. Until it does, reads through it can miss some.
    #[must_use]
    pub fn has_index(&self, group: GroupId, table: ModelId, column: ColumnId) -> bool {
        let held = self.groups.get(&group);

        held.is_some_and(|held| held.has_index(table, column))
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
        let shed = held.shed(&self.identity, None, &mut step).await;
        self.guard(shed)?;

        self.finish(step).await
    }

    /// Sheds the committed history of one clock of `group`, as
    /// [`compact`] does for all of them. The other clocks keep theirs,
    /// and this device can still hand it to a peer that wants only
    /// those.
    ///
    /// [`compact`]: Self::compact
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails.
    pub async fn compact_clock(
        &mut self,
        group: GroupId,
        clock: ClockId,
    ) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let shed = held.shed(&self.identity, Some(clock), &mut step).await;
        self.guard(shed)?;

        self.finish(step).await
    }

    /// Starts getting back the history of `clock` that this device
    /// shed, from any peer that kept it: an archive. Each entry is
    /// checked against what this device kept, so it's as good as
    /// history that never left. It arrives over time; [`is_archive`]
    /// says when it's all here. Compacting again stops it.
    ///
    /// Reads don't change. What changes is that this device can hand
    /// the history on.
    ///
    /// [`is_archive`]: Self::is_archive
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails.
    pub async fn restore(&mut self, group: GroupId, clock: ClockId) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let restoring = held.restore(&self.identity, clock, &mut step).await;
        self.guard(restoring)?;

        self.finish(step).await
    }

    /// Has this device catch up on `group` by being sent what's
    /// committed, in place of every transaction that led to it. It's
    /// the same state either way, and less to send and to check for a
    /// device that's a long way behind. What isn't committed yet still
    /// arrives as transactions.
    ///
    /// It holds until the device has caught up once. The history
    /// skipped isn't kept here: see [`restore`].
    ///
    /// [`restore`]: Self::restore
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails.
    pub async fn skip_ahead(&mut self, group: GroupId) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let skipping = held.skip(&self.identity, &mut step).await;
        self.guard(skipping)?;

        self.finish(step).await
    }

    /// Whether this device holds every entry of `clock` that was ever
    /// committed, and every checkpoint: it shed none, or has them all
    /// back. Such a device can bring a peer up to date entry by entry,
    /// however far behind it is.
    #[must_use]
    pub fn is_archive(&self, group: GroupId, clock: ClockId) -> bool {
        self.groups.get(&group).is_some_and(|it| it.is_whole(clock))
    }

    /// Holds only these clocks of `group`, or with `None`, all of
    /// them, which is how a group starts out. Nothing of the other
    /// clocks is kept, asked for, or sent here, and reads of them come
    /// back empty. Writing to one is refused.
    ///
    /// Wanting less is always possible. Wanting more is possible until
    /// a checkpoint has been committed here without a clock it covers:
    /// so choose straight after [joining].
    ///
    /// [joining]: Self::join
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, has a say in it, or has
    /// passed over a clock it's now asking for, or the store fails.
    pub async fn want(
        &mut self,
        group: GroupId,
        clocks: Option<BTreeSet<ClockId>>,
    ) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let wanted = held.want(&self.identity, clocks, &mut step).await;
        match self.guard(wanted)? {
            Ok(()) => self.finish(step).await,
            Err(group::Refused::Votes) => Err(Error::Votes),
            Err(group::Refused::Passed) => Err(Error::PassedOver),
        }
    }

    /// Asks `peer` for only these clocks of `group`, or with `None`,
    /// for every clock this device holds, which is how a connection
    /// starts out. It lasts until the peer is disconnected, and can be
    /// changed at any time.
    ///
    /// It's for a connection that shouldn't carry everything: a
    /// metered one, or one to a peer trusted with only so much. What's
    /// held back still arrives over other connections, and nothing is
    /// committed here until it has. It narrows what's sent as it's
    /// written, not what [want] has this device hold, and not the
    /// committed state a device that's far behind is sent in place of
    /// history, which comes whole.
    ///
    /// [want]: Self::want
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails. A
    /// peer that isn't connected is asked nothing.
    pub async fn ask(
        &mut self,
        peer: VerifyingKey,
        group: GroupId,
        clocks: Option<BTreeSet<ClockId>>,
    ) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        let asked = held.ask(Peer::from(peer), clocks, &mut step).await;
        self.guard(asked)?;

        self.finish(step).await
    }

    /// Stops holding `group`, and deletes everything kept for it.
    ///
    /// Only a device with no role in the group can. One that could
    /// still write or vote has signed things it would forget, and
    /// would sign them again differently if it [joined] again: it has
    /// to be removed from the group first, by the group.
    ///
    /// [joined]: Self::join
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group` or is still a member of it,
    /// or the store fails.
    pub async fn forget(&mut self, group: GroupId) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get(&group).ok_or(Error::UnknownGroup)?;
        if held.has(self.identity.peer()) {
            return Err(Error::StillAMember);
        }

        let (start, end) = keys::range(keys::group(group));
        let kept = step.scan(&start, &end).await;
        for (key, _) in self.guard(kept)? {
            step.delete(key);
        }
        step.delete(keys::founding(group));
        step.changed(Changed::Everything { group });
        self.groups.remove(&group);

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

    /// The changes in `group` that are waiting to be committed before
    /// they do anything, in the order they'll be tried. Only those
    /// whose transactions count for now are included.
    ///
    /// A [`Changed::Pending`] says when to ask again.
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails.
    pub async fn pending(&self, group: GroupId) -> Result<Vec<Pending>, Error> {
        let held = self.groups.get(&group).ok_or(Error::UnknownGroup)?;
        let step = Step::new(self.store.view());

        Ok(held.pending(&step).await?)
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
        let changes = step.changes();
        for group in self.groups.values_mut() {
            let derived = group.derive(&changes, &mut step).await;
            if derived.is_err() {
                self.broken = true;
            }
            derived?;
        }

        let finished = step.finish();
        let applied = self.store.apply(finished.batch).await;
        self.guard(applied)?;
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
