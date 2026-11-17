//! Reading what a device holds, and the indexes it keeps to read by.

use crate::actor::Peer;
use crate::model::{ColumnId, ModelId, Query};
use crate::store::{Store, View};

use super::group::Group;
use super::step::{self, Step};
use super::veiled::Veiled;
use super::{
    Answer, Database, Error, GroupId, Outcome, Outputs, Part, Pending, Reached, TransactionId,
    keys, parts,
};

impl<S: Store> Database<S> {
    /// Starts keeping an index of `columns` of `table`, one column or
    /// several in order, so that the rows that hold a value in them
    /// can be found with [`Query::index`] or [`Query::index_within`]
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
        columns: impl AsRef<[ColumnId]>,
    ) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        held.index(table, columns.as_ref(), &mut step);

        self.finish(step).await
    }

    /// Stops keeping an index of `columns` of `table`, and throws away
    /// what it held.
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails.
    pub async fn drop_index(
        &mut self,
        group: GroupId,
        table: ModelId,
        columns: impl AsRef<[ColumnId]>,
    ) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let held = self.groups.get_mut(&group).ok_or(Error::UnknownGroup)?;
        held.unindex(table, columns.as_ref(), &mut step);

        self.finish(step).await
    }

    /// Whether this device keeps an index of `columns` of `table` that
    /// has every row. Until it does, reads through it can miss some.
    #[must_use]
    pub fn has_index(
        &self,
        group: GroupId,
        table: ModelId,
        columns: impl AsRef<[ColumnId]>,
    ) -> bool {
        let held = self.groups.get(&group);

        held.is_some_and(|held| held.has_index(table, columns.as_ref()))
    }

    /// What last became of a transaction this device wrote:
    /// the last thing [`Outputs::outcomes`] said of it, for an
    /// application that wasn't there to hear. `None` if this device
    /// didn't write it, or no longer keeps it: [`compact`] sheds what
    /// became of a transaction along with the transaction.
    ///
    /// [`compact`]: Self::compact
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn outcome(&self, id: TransactionId) -> Result<Option<Outcome>, Error> {
        let author = Peer::from(id.author);
        let key = keys::outcome(id.group, id.clock, author, id.seq);
        let stored = self.store.view().get(&key).await?;

        Ok(stored.and_then(|stored| Outcome::from_bytes(&stored)))
    }

    /// Answers `query` from whole steps: all of a step's writes, or none.
    /// The answer says which it is: everything known, or only what's
    /// committed. A query of a model that shows only what's committed
    /// gets the second without asking.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn read(&self, query: &Query) -> Result<Answer, Error> {
        let view = self.view(query);
        let mut scanned = Vec::new();
        for (start, end) in query.storage_ranges() {
            scanned.extend(step::scan(&view, &start, &end).await?);
        }

        let found = query.results(&scanned);

        Ok(if query.is_committed() {
            Answer::Committed(found, self.reached(query))
        } else {
            Answer::Optimistic(found)
        })
    }

    /// Answers `query` up to `limit` of what it asks for, and says how
    /// to ask for the rest. Use it for anything that might be large.
    ///
    /// What's counted is what the query is of: keys, or a [table]'s
    /// rows, or the rows an index finds. A row comes with all its
    /// cells or not at all, and a table's columns come with every
    /// part.
    ///
    /// Each part is read from whole steps, but two parts may be read
    /// either side of one. Whatever that step changed is in its
    /// [`Changed`].
    ///
    /// [table]: crate::ModelId::table
    /// [`Changed`]: crate::Changed
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn read_some(&self, query: &Query, limit: usize) -> Result<Part, Error> {
        let (found, rest) = parts::read(&self.view(query), query, limit).await?;
        let answer = if query.is_committed() {
            Answer::Committed(found, self.reached(query))
        } else {
            Answer::Optimistic(found)
        };

        Ok(Part {
            answer,
            rest: rest.map(|from| query.rest(from)),
        })
    }

    /// How far what's committed has got in the group `query` reads.
    fn reached(&self, query: &Query) -> Reached {
        let held = self.groups.get(&query.group());

        held.map_or(Reached { checkpoint: 0 }, Group::committed_at)
    }

    /// The store as a reader of `query` is meant to see it: without
    /// any of a checkpoint that's partly in place.
    fn view(&self, query: &Query) -> Veiled<S::View> {
        let held = self.groups.get(&query.group());

        Veiled::new(self.store.view(), held.and_then(Group::shown))
    }

    /// The changes in `group` that are waiting to be committed before
    /// they do anything, in the order they'll be tried. Only those
    /// whose transactions count for now are included.
    ///
    /// A [`Changed::Pending`] says when to ask again.
    ///
    /// [`Changed::Pending`]: crate::Changed::Pending
    ///
    /// # Errors
    ///
    /// If this device doesn't hold `group`, or the store fails.
    pub async fn pending(&self, group: GroupId) -> Result<Vec<Pending>, Error> {
        let held = self.groups.get(&group).ok_or(Error::UnknownGroup)?;
        let step = Step::new(self.store.view());

        Ok(held.pending(&step).await?)
    }
}
