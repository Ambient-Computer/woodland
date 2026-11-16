//! A store kept in one file, for anywhere with a filesystem.
//!
//! The file is a [redb] database: a copy-on-write B-tree that commits
//! by swapping its root, so a batch is in the file whole or not at
//! all, however the process ends. A commit returns once it's synced.
//!
//! The file is locked while it's open. A second store on the same
//! file is refused, in this process or another, because two cores on
//! one store are one peer saying two things.
//!
//! Calls answer at once: they block until the disk does. A step is
//! short, and the core does nothing else while it waits anyway.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use redb::{Database, ReadOnlyTable, ReadableDatabase, TableDefinition};

use super::{Entry, Order, Store, StoreError, View, Write};

/// Everything is in one table, keyed by bytes.
const TABLE: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("woodland");

type Table = ReadOnlyTable<&'static [u8], &'static [u8]>;

pub struct FileStore {
    database: Database,
}

impl fmt::Debug for FileStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("FileStore").finish_non_exhaustive()
    }
}

/// The file as it was when the view was taken. It keeps that much of
/// the file from being reused, so it shouldn't be kept for long.
#[derive(Clone)]
pub struct FileView(Result<Arc<Table>, StoreError>);

impl fmt::Debug for FileView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("FileView").finish_non_exhaustive()
    }
}

fn failed(error: impl Into<redb::Error>) -> StoreError {
    StoreError::new(error.into().to_string())
}

impl FileStore {
    /// Opens the store in the file at `path`, or starts one there.
    ///
    /// # Errors
    ///
    /// If the file can't be opened or isn't a store, or another store
    /// has it open.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let database = Database::create(path).map_err(failed)?;

        // A view reads the table, so it has to be there from the start.
        let creating = database.begin_write().map_err(failed)?;
        creating.open_table(TABLE).map_err(failed)?;
        creating.commit().map_err(failed)?;

        Ok(Self { database })
    }

    fn write(&self, batch: Vec<Write>) -> Result<(), StoreError> {
        let writing = self.database.begin_write().map_err(failed)?;
        {
            let mut table = writing.open_table(TABLE).map_err(failed)?;
            for write in batch {
                match write {
                    Write::Put { key, value } => {
                        table
                            .insert(key.as_slice(), value.as_slice())
                            .map_err(failed)?;
                    }

                    Write::Delete { key } => {
                        table.remove(key.as_slice()).map_err(failed)?;
                    }
                }
            }
        }

        writing.commit().map_err(failed)
    }

    fn read(&self) -> Result<Arc<Table>, StoreError> {
        let reading = self.database.begin_read().map_err(failed)?;
        let table = reading.open_table(TABLE).map_err(failed)?;

        Ok(Arc::new(table))
    }
}

impl Store for FileStore {
    type View = FileView;

    fn apply(&mut self, batch: Vec<Write>) -> impl Future<Output = Result<(), StoreError>> {
        std::future::ready(self.write(batch))
    }

    fn view(&self) -> FileView {
        FileView(self.read())
    }
}

impl FileView {
    fn range(
        &self,
        start: &[u8],
        end: Option<&[u8]>,
        order: Order,
        limit: usize,
    ) -> Result<Vec<Entry>, StoreError> {
        let table = self.0.as_ref().map_err(Clone::clone)?;
        if end.is_some_and(|end| end <= start) {
            return Ok(Vec::new());
        }

        let range = match end {
            Some(end) => table.range::<&[u8]>(start..end),
            None => table.range::<&[u8]>(start..),
        };
        let range = range.map_err(failed)?;
        let entry = |found: Result<_, redb::StorageError>| {
            let (key, value): (redb::AccessGuard<'_, &[u8]>, redb::AccessGuard<'_, &[u8]>) =
                found.map_err(failed)?;

            Ok((key.value().to_vec(), value.value().to_vec()))
        };

        match order {
            Order::Ascending => range.take(limit).map(entry).collect(),
            Order::Descending => range.rev().take(limit).map(entry).collect(),
        }
    }
}

impl View for FileView {
    fn get(&self, key: &[u8]) -> impl Future<Output = Result<Option<Vec<u8>>, StoreError>> {
        let table = self.0.as_ref().map_err(Clone::clone);
        let found = table.and_then(|table| {
            let held = table.get(key).map_err(failed)?;

            Ok(held.map(|value| value.value().to_vec()))
        });

        std::future::ready(found)
    }

    fn scan(
        &self,
        start: &[u8],
        end: Option<&[u8]>,
        order: Order,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<Entry>, StoreError>> {
        std::future::ready(self.range(start, end, order, limit))
    }
}

#[cfg(test)]
mod tests;
