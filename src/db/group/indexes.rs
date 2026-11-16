//! Indexes: which rows hold a value in a column, kept beside the rows.
//!
//! An index is this device's own. No transaction writes it, and no
//! other device knows it's there. It's worked out from the rows, at
//! both levels of read, and kept up as they change: whatever a step
//! reports as changed has its place in each index worked out again,
//! in the same step.
//!
//! A new index is filled in a page of rows at a time, and so is one
//! that has to start again because the rows were replaced wholesale.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::db::{Changed, keys};
use crate::encoding;
use crate::model::table::{ColumnId, Place, Value};
use crate::model::{ModelId, Query};
use crate::store::{StoreError, View};

use super::super::step::Step;
use super::{Group, Shown};

/// About how many keys one part of filling an index looks at.
const PAGE: usize = if cfg!(test) { 6 } else { 256 };

/// How far along an index is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Build {
    /// Throwing away what it held, to start again.
    Clearing,
    /// Going through the rows that are committed.
    Committed { after: Option<Vec<u8>> },
    /// Going through the rows that aren't.
    Shown { after: Option<Vec<u8>> },
    /// It has every row.
    Ready,
    /// Throwing away what it held, for good.
    Dropping,
}

/// The indexes a device keeps: of which columns of which table, in
/// which order.
pub(super) type Indexes = BTreeMap<(ModelId, Vec<ColumnId>), Build>;

impl Group {
    pub(super) async fn recall_indexes<V: View>(
        &mut self,
        step: &Step<V>,
    ) -> Result<(), StoreError> {
        let held = step.load::<Vec<_>>(&keys::indexes(self.id)).await?;
        self.indexes = held.unwrap_or_default().into_iter().collect();

        Ok(())
    }

    fn keep_indexes<V: View>(&self, step: &mut Step<V>) {
        let held: Vec<_> = self.indexes.iter().collect();
        step.save(keys::indexes(self.id), &held);
    }

    /// Starts keeping an index of `columns`, if there isn't one. No
    /// columns, or more than can be counted in a byte, is no index.
    pub(crate) fn index<V: View>(
        &mut self,
        table: ModelId,
        columns: &[ColumnId],
        step: &mut Step<V>,
    ) {
        if columns.is_empty() || u8::try_from(columns.len()).is_err() {
            return;
        }

        let held = self.indexes.get(&(table, columns.to_vec()));
        if held.is_none_or(|build| *build == Build::Dropping) {
            self.indexes
                .insert((table, columns.to_vec()), Build::Clearing);
            self.keep_indexes(step);
        }
    }

    /// Stops keeping an index of `columns`.
    pub(crate) fn unindex<V: View>(
        &mut self,
        table: ModelId,
        columns: &[ColumnId],
        step: &mut Step<V>,
    ) {
        if let Some(build) = self.indexes.get_mut(&(table, columns.to_vec())) {
            *build = Build::Dropping;
            self.keep_indexes(step);
        }
    }

    /// Whether there's an index of `columns` that has every row.
    pub(crate) fn has_index(&self, table: ModelId, columns: &[ColumnId]) -> bool {
        self.indexes.get(&(table, columns.to_vec())) == Some(&Build::Ready)
    }

    /// Whether an index is still being filled or thrown away.
    pub(super) fn is_indexing(&self) -> bool {
        self.indexes.values().any(|build| *build != Build::Ready)
    }

    /// Brings the indexes up to date with what the step changed, and
    /// carries on with one that isn't full.
    pub(crate) async fn derive<V: View>(
        &mut self,
        changes: &[Changed],
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        if self.indexes.is_empty() {
            return Ok(());
        }

        let everything = Changed::Everything { group: self.id };
        if changes.contains(&everything) {
            for build in self.indexes.values_mut() {
                if *build != Build::Dropping {
                    *build = Build::Clearing;
                }
            }
            self.keep_indexes(step);
        } else {
            for change in changes {
                self.follow(change, step).await?;
            }
        }

        self.fill(step).await?;
        #[cfg(test)]
        self.check_indexes(step).await?;

        Ok(())
    }

    /// Holds every index that's full to what reading the whole table
    /// says it should be.
    #[cfg(test)]
    async fn check_indexes<V: View>(&self, step: &Step<V>) -> Result<(), StoreError> {
        let ready = self
            .indexes
            .iter()
            .filter(|(_, build)| **build == Build::Ready);
        for ((table, columns), _) in ready {
            let table = *table;
            for committed in [false, true] {
                let query = Query::table(self.id, table);
                let query = if committed { query.committed() } else { query };
                let mut scanned = Vec::new();
                for (start, end) in query.storage_ranges() {
                    scanned.extend(step.scan(&start, &end).await?);
                }
                let read = crate::Table::read(&query.results(&scanned));
                let named: Vec<_> = read.iter().flat_map(crate::Table::columns).collect();
                let named = columns
                    .iter()
                    .all(|column| named.iter().any(|(id, _)| id == column));
                let rows = read.iter().flat_map(crate::Table::rows);
                let held = |row: &crate::Row| -> Vec<Value> {
                    let values = columns.iter().map(|&column| row.get(column).clone());

                    values.filter(|value| *value != Value::Null).collect()
                };
                let mut expected: Vec<_> = rows
                    .map(|row| (row, held(row)))
                    .filter(|(_, held)| named && held.len() == columns.len())
                    .map(|(row, held)| Place::index(columns, &held, row.key()))
                    .collect();
                expected.sort();

                let prefix = keys::index(self.id, committed, table, &Place::index_of(columns));
                let (start, end) = keys::range(prefix);
                let found = step.scan(&start, &end).await?;
                let placed = |(key, _): (Vec<u8>, Vec<u8>)| match keys::parse(&key) {
                    Some(keys::Record::Index { key, .. }) => Some(key),
                    _ => None,
                };
                let found: Vec<_> = found.into_iter().filter_map(placed).collect();
                assert_eq!(
                    found, expected,
                    "an index kept up as rows changed isn't the one worked out from the table (committed: {committed})"
                );
            }
        }

        Ok(())
    }

    /// Works out again where a changed key leaves its row in each
    /// index.
    async fn follow<V: View>(
        &mut self,
        change: &Changed,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let Changed::Key {
            group,
            model,
            key,
            committed,
        } = change
        else {
            return Ok(());
        };
        if *group != self.id {
            return Ok(());
        }
        let (row, only) = match Place::parse(key) {
            Some(Place::Row(row)) => (row, None),
            Some(Place::Cell(row, _, column)) => (row, Some(column)),
            // Every row is in or out of an index with each of its
            // columns.
            Some(Place::Column(column)) => {
                let of = |((table, columns), build): &(&(ModelId, Vec<ColumnId>), &mut Build)| {
                    table == model && columns.contains(&column) && **build != Build::Dropping
                };
                let mut cleared = false;
                for (_, build) in self.indexes.iter_mut().filter(of) {
                    *build = Build::Clearing;
                    cleared = true;
                }

                if cleared {
                    self.keep_indexes(step);
                }

                return Ok(());
            }
            _ => return Ok(()),
        };

        let filling = |build: &Build| !matches!(build, Build::Clearing | Build::Dropping);
        let indexed = self.indexes.iter();
        let indexed = indexed.filter(|((table, _), build)| table == model && filling(build));
        let indexed = indexed.map(|((_, columns), _)| columns);
        let touched = indexed.filter(|columns| only.is_none_or(|only| columns.contains(&only)));
        for columns in touched.cloned().collect::<Vec<_>>() {
            // What's committed shows through to an optimistic read.
            self.file(*model, &columns, &row, false, step).await?;
            if *committed {
                self.file(*model, &columns, &row, true, step).await?;
            }
        }

        Ok(())
    }

    /// What a key holds, to one level of read: as it's stored, or as
    /// a reader is `shown` it while a checkpoint is partly in place.
    async fn held<V: View>(
        &self,
        model: ModelId,
        key: &[u8],
        (committed, shown): (bool, Option<&Shown>),
        step: &Step<V>,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let query = Query::key(self.id, model, key);
        let query = if committed { query.committed() } else { query };
        let mut scanned = Vec::new();
        for (start, end) in query.storage_ranges() {
            scanned.extend(match shown {
                Some(Shown {
                    prefix,
                    before,
                    blank,
                }) => {
                    let range = (&start[..], &end[..]);

                    step.scan_shown(range, prefix, before, *blank).await?
                }
                None => step.scan(&start, &end).await?,
            });
        }

        Ok(query
            .results(&scanned)
            .into_iter()
            .next()
            .map(|(_, value)| value))
    }

    /// What a row holds in each of some columns, at one level of
    /// read. `None` unless the row is there and holds a value in
    /// every one of them.
    async fn values<V: View>(
        &self,
        table: ModelId,
        columns: &[ColumnId],
        row: &[u8],
        level: (bool, Option<&Shown>),
        step: &Step<V>,
    ) -> Result<Option<Vec<Value>>, StoreError> {
        let exists = Place::Row(row.to_vec()).key();
        let incarnation = self.held(table, &exists, level, step).await?;
        let Some(incarnation) = incarnation.and_then(|held| held.try_into().ok()) else {
            return Ok(None);
        };

        let mut values = Vec::new();
        for &column in columns {
            let named = Place::Column(column).key();
            if self.held(table, &named, level, step).await?.is_none() {
                return Ok(None);
            }

            let cell = Place::Cell(row.to_vec(), incarnation, column).key();
            let held = self.held(table, &cell, level, step).await?;
            let value = held.and_then(|held| encoding::decode::<Value>(&held));
            match value.filter(|value| *value != Value::Null) {
                Some(value) => values.push(value),
                None => return Ok(None),
            }
        }

        Ok(Some(values))
    }

    /// Puts a row where it belongs in an index, at one level of read:
    /// under the values it holds, if it's there and holds them all.
    async fn file<V: View>(
        &self,
        table: ModelId,
        columns: &[ColumnId],
        row: &[u8],
        committed: bool,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let level = (committed, None);
        let values = self.values(table, columns, row, level, step).await?;
        let entry = values.map(|values| Place::index(columns, &values, row));

        let of = Place::indexed(columns);
        let whereabouts = keys::indexed(self.id, committed, table, &of, row);
        let was = step.get(&whereabouts).await?;
        if let Some(shown) = self.shown().filter(|_| !committed) {
            let places = (was.clone(), entry.clone());
            self.file_shown((table, columns), row, places, &shown, step)
                .await?;
        }

        if was == entry {
            return Ok(());
        }
        // A query through the index is told like one of the table: by
        // the key that changed.
        let moved = |key: &[u8]| Changed::Key {
            group: self.id,
            model: table,
            key: key.to_vec(),
            committed,
        };
        if let Some(was) = was {
            step.changed(moved(&was));
            step.delete(keys::index(self.id, committed, table, &was));
        }

        match entry {
            Some(entry) => {
                step.changed(moved(&entry));
                step.put(keys::index(self.id, committed, table, &entry), Vec::new());
                step.put(whereabouts, entry);
            }

            None => step.delete(whereabouts),
        }

        Ok(())
    }

    /// Has readers find a row where it belongs in an index of what's
    /// shown, while a checkpoint is partly in place: where what they're
    /// shown of the row puts it, which needn't be where what's stored
    /// does. `places` are where the index itself had the row, and has
    /// it now.
    ///
    /// The index is kept right for what's stored, so that it's right
    /// the moment the checkpoint shows. What readers find meanwhile
    /// is kept beside it, like every value the checkpoint replaces
    /// (see [`Step::keep`]), a place at a time: here or not here.
    async fn file_shown<V: View>(
        &self,
        (table, columns): (ModelId, &[ColumnId]),
        row: &[u8],
        (was, entry): (Option<Vec<u8>>, Option<Vec<u8>>),
        shown: &Shown,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let beside = |key: Vec<u8>| [&shown.before[..], &key[shown.prefix.len()..]].concat();
        let level = (false, Some(shown));
        let values = self.values(table, columns, row, level, step).await?;
        let seen = values.map(|values| Place::index(columns, &values, row));

        // Where readers last found it: where this said, or where it
        // was when the checkpoint began.
        let of = Place::indexed(columns);
        let found = beside(keys::indexed(self.id, false, table, &of, row));
        let last = step.load::<Option<Vec<u8>>>(&found).await?;
        let last = last.unwrap_or_else(|| was.clone());
        step.save(found, &seen);

        let places: BTreeSet<_> = [was, entry, last, seen.clone()]
            .into_iter()
            .flatten()
            .collect();
        for place in places {
            let here = (Some(&place) == seen.as_ref()).then(Vec::<u8>::new);
            let key = keys::index(self.id, false, table, &place);
            step.save(beside(key), &here);
            step.changed(Changed::Key {
                group: self.id,
                model: table,
                key: place,
                committed: false,
            });
        }

        Ok(())
    }

    /// Does a part of filling, or throwing away, the first index that
    /// needs it.
    async fn fill<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        let unfinished = |(_, build): &(&(ModelId, Vec<ColumnId>), &Build)| **build != Build::Ready;
        let Some(((table, columns), build)) = self.indexes.iter().find(unfinished) else {
            return Ok(());
        };

        let (table, columns) = (*table, columns.clone());

        let (first, last) = Place::rows();
        let next = match build.clone() {
            Build::Ready => return Ok(()),
            Build::Clearing | Build::Dropping => {
                let mut cleared = true;
                for committed in [false, true] {
                    let entries = Place::index_of(&columns);
                    let entries = keys::index(self.id, committed, table, &entries);
                    let of = Place::indexed(&columns);
                    let places = keys::indexed(self.id, committed, table, &of, b"");
                    for prefix in [entries, places] {
                        let (start, end) = keys::range(prefix);
                        let page = step.scan_page(&start, &end, PAGE).await?;
                        cleared &= page.len() < PAGE;
                        for (key, _) in page {
                            step.delete(key);
                        }
                    }
                }

                match (cleared, build) {
                    (false, build) => Some(build.clone()),
                    (true, Build::Dropping) => None,
                    (true, _) => Some(Build::Committed { after: None }),
                }
            }

            Build::Committed { after } => {
                let start = keys::committed_value(self.id, table, &first);
                let start = after.as_deref().map_or(start, keys::after);
                let end = keys::committed_value(self.id, table, &last);
                let page = step.scan_page(&start, &end, PAGE).await?;
                for (key, _) in &page {
                    if let Some(row) = keys::parse(key).and_then(keys::Record::row) {
                        self.file(table, &columns, &row, false, step).await?;
                        self.file(table, &columns, &row, true, step).await?;
                    }
                }

                let after = page.last().map(|(key, _)| key.clone());
                if page.len() < PAGE {
                    Some(Build::Shown { after: None })
                } else {
                    Some(Build::Committed { after })
                }
            }

            Build::Shown { after } => {
                let start = keys::versions(self.id, table, &first);
                let start = after.as_deref().map_or(start, keys::after);
                let end = keys::versions(self.id, table, &last);
                let page = step.scan_page(&start, &end, PAGE).await?;
                for (key, _) in &page {
                    if let Some(row) = keys::parse(key).and_then(keys::Record::row) {
                        self.file(table, &columns, &row, false, step).await?;
                    }
                }

                let after = page.last().map(|(key, _)| key.clone());
                if page.len() < PAGE {
                    step.changed(Changed::Indexed {
                        group: self.id,
                        model: table,
                        columns: columns.clone(),
                    });

                    Some(Build::Ready)
                } else {
                    Some(Build::Shown { after })
                }
            }
        };

        match next {
            Some(next) => self.indexes.insert((table, columns), next),
            None => self.indexes.remove(&(table, columns)),
        };
        self.keep_indexes(step);

        Ok(())
    }
}
