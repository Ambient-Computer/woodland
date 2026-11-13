use std::collections::BTreeMap;

use super::place::{Incarnation, Place};
use super::rows;
use super::{ColumnId, Operation, Rekey, Target, Value};
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

/// One row of a [`Table`], as it was when it was read. Changing or
/// deleting a row starts from one of these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    key: Vec<u8>,
    incarnation: Incarnation,
    /// What it holds in the table's columns.
    cells: BTreeMap<ColumnId, Value>,
    /// What it holds in unique columns, dropped ones included, exactly
    /// as stored.
    keys: Vec<(ColumnId, Value)>,
    /// What it's stored under each cell it has, dropped columns
    /// included.
    stored: BTreeMap<ColumnId, Fingerprint>,
    /// The columns the table had.
    columns: Vec<ColumnId>,
}

/// A row's cells, as stored.
type Stored = Vec<(ColumnId, Vec<u8>)>;

/// The parts of a table's rows, as they come out of storage.
#[derive(Default)]
struct Parts {
    rows: BTreeMap<Vec<u8>, Incarnation>,
    keys: BTreeMap<Vec<u8>, Vec<(ColumnId, Value)>>,
    cells: BTreeMap<(Vec<u8>, Incarnation), Stored>,
}

impl Table {
    /// Decodes a table from the results of a query for all of it.
    /// `None` if there's no such table: it was never created, its
    /// creation didn't happen, or it was dropped.
    #[must_use]
    pub fn read(results: &[(Vec<u8>, Vec<u8>)]) -> Option<Self> {
        let mut exists = false;
        let mut columns = BTreeMap::new();
        let mut parts = Parts::default();
        for (key, value) in results {
            match Place::parse(key) {
                Some(Place::Table) => exists = true,
                Some(Place::Column(id)) => {
                    columns.insert(id, String::from_utf8_lossy(value).into_owned());
                }
                Some(Place::Row(key)) => {
                    if let Ok(incarnation) = value.as_slice().try_into() {
                        parts.rows.insert(key, incarnation);
                    }
                }
                Some(Place::Keys(key)) => {
                    let keys: Option<BTreeMap<ColumnId, Value>> = encoding::decode(value);
                    let keys = keys.unwrap_or_default();
                    parts.keys.insert(key, keys.into_iter().collect());
                }
                Some(Place::Cell(key, incarnation, column)) => {
                    let cells = parts.cells.entry((key, incarnation)).or_default();
                    cells.push((column, value.clone()));
                }
                Some(Place::Added(_) | Place::Unique(..) | Place::Index(..)) | None => {}
            }
        }

        let rows = parts.rows(&columns);

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

    /// Decodes the keys of the rows an index query found, in the order
    /// of the values they hold in its column. See [`Query::index`].
    ///
    /// [`Query::index`]: crate::Query::index
    #[must_use]
    pub fn row_keys(results: &[(Vec<u8>, Vec<u8>)]) -> Vec<Vec<u8>> {
        let row = |(key, value): &(Vec<u8>, Vec<u8>)| match Place::parse(key)? {
            Place::Unique(..) => Some(value.clone()),
            Place::Index(_, _, row) => Some(row),
            _ => None,
        };

        results.iter().filter_map(row).collect()
    }

    /// The first row that holds `value` in `column`. If the column is
    /// [unique], the only one.
    ///
    /// [unique]: ColumnId::unique
    #[must_use]
    pub fn find(&self, column: ColumnId, value: &Value) -> Option<&Row> {
        self.rows.iter().find(|row| row.get(column) == value)
    }
}

impl Parts {
    /// Puts each row together: the cells of the incarnation that
    /// exists, and what it holds in unique columns. A cell of a column
    /// that's since been dropped is still stored, and nobody should
    /// see it.
    fn rows(mut self, columns: &BTreeMap<ColumnId, String>) -> Vec<Row> {
        let rows = std::mem::take(&mut self.rows);
        let row = |(key, incarnation): (Vec<u8>, Incarnation)| {
            let keys = self.keys.remove(&key).unwrap_or_default();
            let raw = self.cells.remove(&(key.clone(), incarnation));
            let raw = raw.unwrap_or_default();
            let stored = raw
                .iter()
                .map(|(column, value)| (*column, Fingerprint::of(value)));
            let stored = stored.collect();

            let plain = raw.iter().filter_map(|(column, value)| {
                let value = encoding::decode::<Value>(value)?;

                Some((*column, value))
            });

            let mut cells: BTreeMap<_, _> = plain.chain(keys.iter().cloned()).collect();
            cells.retain(|column, _| columns.contains_key(column));

            Row {
                key,
                incarnation,
                cells,
                keys,
                stored,
                columns: columns.keys().copied().collect(),
            }
        };

        rows.into_iter().map(row).collect()
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

    fn target(&self) -> Target {
        Target {
            key: self.key.clone(),
            incarnation: self.incarnation,
        }
    }

    /// The operation that gives some of the row's columns new values.
    /// A unique column's change goes with everything the row held in
    /// unique columns, as it was read.
    pub(crate) fn set(&self, changes: impl IntoIterator<Item = (ColumnId, Value)>) -> Operation {
        let (unique, cells): (Vec<_>, Vec<_>) = changes
            .into_iter()
            .partition(|(column, _)| column.is_keyed());

        let rekey = (!unique.is_empty()).then(|| {
            let mut to: BTreeMap<_, _> = self.keys.iter().cloned().collect();
            for (column, value) in unique {
                if value == Value::Null {
                    to.remove(&column);
                } else {
                    to.insert(column, value);
                }
            }

            Rekey {
                from: self.keys.clone(),
                to: to.into_iter().collect(),
            }
        });

        Operation::Set {
            row: self.target(),
            cells,
            rekey,
        }
    }

    /// The operation that removes the row, and everything it was read
    /// to hold.
    pub(crate) fn delete(&self) -> Operation {
        Operation::Delete {
            row: self.target(),
            cells: self.stored.keys().copied().collect(),
            keys: self.keys.clone(),
        }
    }

    /// What it takes for the row to still be exactly what was read:
    /// the same incarnation, and the same thing, or nothing, in every
    /// column the table had.
    pub(crate) fn unchanged(&self) -> Vec<(Vec<u8>, Requirement)> {
        let plain = self.columns.iter().filter(|column| !column.is_keyed());
        let cells = plain.map(|column| {
            let key = Place::Cell(self.key.clone(), self.incarnation, *column).key();
            let requirement = match self.stored.get(column) {
                Some(stored) => Requirement::Is(*stored),
                None => Requirement::Vacant,
            };

            (key, requirement)
        });

        let mut requires = rows::delete_requires(&self.target(), &self.keys);
        requires.extend(cells);
        requires
    }
}
