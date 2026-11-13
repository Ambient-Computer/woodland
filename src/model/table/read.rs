use std::collections::BTreeMap;

use super::{ColumnId, Place, Value};
use crate::encoding;
use crate::model::{Fingerprint, Requirement};

/// A table as it's read: its columns, and its rows in key order.
///
/// Read the whole table with [`Query::table`], and decode what
/// [`Query::results`] returns.
///
/// [`Query::table`]: crate::Query::table
/// [`Query::results`]: crate::Query::results
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    columns: BTreeMap<ColumnId, String>,
    rows: Vec<Row>,
}

/// One row of a [`Table`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    key: Vec<u8>,
    cells: BTreeMap<ColumnId, Value>,
    /// Stands for the row exactly as it was stored.
    stored: Fingerprint,
}

impl Table {
    /// Decodes a table from the results of a query for all of it.
    /// `None` if there's no such table: it was never created, its
    /// creation didn't happen, or it was dropped.
    #[must_use]
    pub fn read(results: &[(Vec<u8>, Vec<u8>)]) -> Option<Self> {
        let mut exists = false;
        let mut columns = BTreeMap::new();
        let mut rows = Vec::new();
        for (key, value) in results {
            match Place::parse(key) {
                Some(Place::Table) => exists = true,
                Some(Place::Column(id)) => {
                    columns.insert(id, String::from_utf8_lossy(value).into_owned());
                }
                Some(Place::Row(key)) => {
                    if let Some(cells) = encoding::decode(value) {
                        let stored = Fingerprint::of(value);
                        rows.push(Row { key, cells, stored });
                    }
                }
                Some(Place::Added(_)) | None => {}
            }
        }

        // A row keeps what it was given for a column that's since been
        // dropped. Nobody should see it.
        for row in &mut rows {
            row.cells.retain(|column, _| columns.contains_key(column));
        }

        exists.then_some(Self { columns, rows })
    }

    /// The table's columns and their names, in no meaningful order.
    pub fn columns(&self) -> impl Iterator<Item = (ColumnId, &str)> {
        self.columns.iter().map(|(&id, name)| (id, name.as_str()))
    }

    /// The rows, in key order.
    #[must_use]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The row with this key, if there is one.
    #[must_use]
    pub fn row(&self, key: &[u8]) -> Option<&Row> {
        self.rows.iter().find(|row| row.key == key)
    }
}

impl Row {
    #[must_use]
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    /// What the row holds in `column`. [`Value::Null`] if it was never
    /// given anything.
    #[must_use]
    pub fn get(&self, column: ColumnId) -> &Value {
        self.cells.get(&column).unwrap_or(&Value::Null)
    }

    /// The row's key in its model, and the requirement that it's still
    /// what was read.
    pub(crate) fn unchanged(&self) -> (Vec<u8>, Requirement) {
        let key = Place::Row(self.key.clone()).key();

        (key, Requirement::Is(self.stored))
    }
}
