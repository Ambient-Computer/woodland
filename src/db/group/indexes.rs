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

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::db::{Changed, keys};
use crate::encoding;
use crate::model::table::{ColumnId, Place, Value};
use crate::model::{ModelId, Query};
use crate::store::{StoreError, View};

use super::super::step::Step;
use super::Group;

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

pub(super) type Indexes = BTreeMap<(ModelId, ColumnId), Build>;

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

    /// Starts keeping an index of `column`, if there isn't one.
    pub(crate) fn index<V: View>(&mut self, table: ModelId, column: ColumnId, step: &mut Step<V>) {
        let held = self.indexes.get(&(table, column));
        if held.is_none_or(|build| *build == Build::Dropping) {
            self.indexes.insert((table, column), Build::Clearing);
            self.keep_indexes(step);
        }
    }

    /// Stops keeping an index of `column`.
    pub(crate) fn unindex<V: View>(
        &mut self,
        table: ModelId,
        column: ColumnId,
        step: &mut Step<V>,
    ) {
        if let Some(build) = self.indexes.get_mut(&(table, column)) {
            *build = Build::Dropping;
            self.keep_indexes(step);
        }
    }

    /// Whether there's an index of `column` that has every row.
    pub(crate) fn has_index(&self, table: ModelId, column: ColumnId) -> bool {
        self.indexes.get(&(table, column)) == Some(&Build::Ready)
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
        for (&(table, column), _) in ready {
            for committed in [false, true] {
                let query = Query::table(self.id, table);
                let query = if committed { query.committed() } else { query };
                let mut scanned = Vec::new();
                for (start, end) in query.storage_ranges() {
                    scanned.extend(step.scan(&start, &end).await?);
                }
                let read = crate::Table::read(&query.results(&scanned));
                let named = read.iter().flat_map(crate::Table::columns);
                let named = named.into_iter().any(|(id, _)| id == column);
                let rows = read.iter().flat_map(crate::Table::rows);
                let holding = rows.filter(|row| named && *row.get(column) != Value::Null);
                let mut expected: Vec<_> = holding
                    .map(|row| Place::index(column, row.get(column), row.key()))
                    .collect();
                expected.sort();

                let prefix = keys::index(self.id, committed, table, &Place::index_of(column));
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
            // Every row is in or out of the index with its column.
            Some(Place::Column(column)) => {
                let held = self.indexes.get_mut(&(*model, column));
                if let Some(build) = held.filter(|build| **build != Build::Dropping) {
                    *build = Build::Clearing;
                    self.keep_indexes(step);
                }

                return Ok(());
            }
            _ => return Ok(()),
        };

        let filling = |build: &Build| !matches!(build, Build::Clearing | Build::Dropping);
        let table = (*model, ColumnId::FIRST)..=(*model, ColumnId::LAST);
        let indexed = self
            .indexes
            .range(table)
            .filter(|(_, build)| filling(build));
        let columns = indexed.map(|((_, column), _)| *column);
        for column in columns.filter(|column| only.is_none_or(|only| only == *column)) {
            // What's committed shows through to an optimistic read.
            self.file(*model, column, &row, false, step).await?;
            if *committed {
                self.file(*model, column, &row, true, step).await?;
            }
        }

        Ok(())
    }

    /// What a key holds, to one level of read.
    async fn held<V: View>(
        &self,
        model: ModelId,
        key: &[u8],
        committed: bool,
        step: &Step<V>,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let query = Query::key(self.id, model, key);
        let query = if committed { query.committed() } else { query };
        let mut scanned = Vec::new();
        for (start, end) in query.storage_ranges() {
            scanned.extend(step.scan(&start, &end).await?);
        }

        Ok(query
            .results(&scanned)
            .into_iter()
            .next()
            .map(|(_, value)| value))
    }

    /// Puts a row where it belongs in an index, at one level of read:
    /// under the value it holds, if it's there and holds one.
    async fn file<V: View>(
        &self,
        table: ModelId,
        column: ColumnId,
        row: &[u8],
        committed: bool,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let exists = Place::Row(row.to_vec()).key();
        let incarnation = self.held(table, &exists, committed, step).await?;
        let incarnation = incarnation.and_then(|held| held.try_into().ok());
        let named = Place::Column(column).key();
        let named = self.held(table, &named, committed, step).await?.is_some();
        let incarnation = incarnation.filter(|_| named);
        let value = match incarnation {
            Some(incarnation) => {
                let cell = Place::Cell(row.to_vec(), incarnation, column).key();
                let held = self.held(table, &cell, committed, step).await?;

                held.and_then(|held| encoding::decode::<Value>(&held))
            }

            None => None,
        };
        let value = value.filter(|value| *value != Value::Null);
        let entry = value.map(|value| Place::index(column, &value, row));

        let whereabouts = keys::indexed(self.id, committed, table, column.as_bytes(), row);
        let was = step.get(&whereabouts).await?;
        if was == entry {
            return Ok(());
        }
        if let Some(was) = was {
            step.delete(keys::index(self.id, committed, table, &was));
        }

        match entry {
            Some(entry) => {
                step.put(keys::index(self.id, committed, table, &entry), Vec::new());
                step.put(whereabouts, entry);
            }

            None => step.delete(whereabouts),
        }

        Ok(())
    }

    /// Does a part of filling, or throwing away, the first index that
    /// needs it.
    async fn fill<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        let unfinished = |(_, build): &(&(ModelId, ColumnId), &Build)| **build != Build::Ready;
        let Some((&(table, column), build)) = self.indexes.iter().find(unfinished) else {
            return Ok(());
        };

        let (first, last) = Place::rows();
        let next = match build.clone() {
            Build::Ready => return Ok(()),
            Build::Clearing | Build::Dropping => {
                let mut cleared = true;
                for committed in [false, true] {
                    let entries = Place::index_of(column);
                    let entries = keys::index(self.id, committed, table, &entries);
                    let places = keys::indexed(self.id, committed, table, column.as_bytes(), b"");
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
                        self.file(table, column, &row, false, step).await?;
                        self.file(table, column, &row, true, step).await?;
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
                        self.file(table, column, &row, false, step).await?;
                    }
                }

                let after = page.last().map(|(key, _)| key.clone());
                if page.len() < PAGE {
                    step.changed(Changed::Indexed {
                        group: self.id,
                        model: table,
                        column,
                    });

                    Some(Build::Ready)
                } else {
                    Some(Build::Shown { after })
                }
            }
        };

        match next {
            Some(next) => self.indexes.insert((table, column), next),
            None => self.indexes.remove(&(table, column)),
        };
        self.keep_indexes(step);

        Ok(())
    }
}
