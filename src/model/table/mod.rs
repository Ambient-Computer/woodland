//! Tables: rows of typed cells, under a schema that's part of the data.
//!
//! A table is a model like any other. Everything about it is in the
//! timeline: creating it, its columns, and its rows are all written by
//! operations, in transactions, and the rules that keep it coherent are
//! the two requirements every model has (see [`Requirement`]):
//!
//! - **Vacant** is how only one transaction gets something. Creating a
//!   table requires that it doesn't exist. Inserting a row requires that
//!   no row has its key. Of two transactions that try, the first in the
//!   timeline happens and the other is pruned, on every device.
//! - **Present** is how a transaction depends on another. Writing a
//!   row requires its table, and every column it has a value for.
//!   Replacing a row requires the row. If the transaction that created
//!   the table was pruned, then so is every transaction that wrote to
//!   it, however many there were and whoever wrote them.
//!
//! # Rows
//!
//! A row is one value: all of its cells. Writing one replaces whatever
//! was there, and of two transactions that write the same row, the
//! later in the timeline is what's read. So changing one column means
//! writing the row again with the others as they were read, and a
//! concurrent change to another column is lost to it, or wins over it.
//! Merging the two would take a write that depends on the value under
//! it, which nothing here has yet.
//!
//! So a device can create a table and fill it while offline, and none
//! of it is final until it settles. Until then it either all stands, or
//! goes together.
//!
//! # Schema
//!
//! Columns are added, renamed and dropped by operations, like rows are
//! written. Each column is its own key, so two devices that each add a
//! column both succeed, and a write that names a column depends on
//! exactly that column: one written after the column was dropped, in
//! the timeline, is pruned.
//!
//! A row written before a column was dropped keeps what it was given
//! for it, and readers leave that out. Nothing goes back to rewrite
//! rows. A dropped column can't be added again, so that never
//! resurfaces.
//!
//! # Unique columns
//!
//! A column can be unique: no two rows hold the same value in it. See
//! [`unique`] for how, and for why a table with one only takes changes
//! to rows from whoever read them.
//!
//! # What's stored
//!
//! A few kinds of key (see [`Place`]): one that says the table exists,
//! one per column holding its name, and one per row holding its cells.
//! A requirement is about one key, which is why each of those is one.
//!
//! # Types
//!
//! A column's type is part of its ID (see [`ColumnId`]), so a cell is
//! checked against its column without reading anything. An operation
//! with a value of the wrong type isn't one a table takes, and its
//! transaction is pruned like any other that doesn't belong.
//!
//! # What isn't here
//!
//! Operations name the rows they change. There's no "every row where":
//! what that touches depends on the table's state at the transaction's
//! place in the timeline, which is a scan, and nothing that decides
//! whether transactions happen reads more than keys. Filter when
//! reading, and write to the rows found.

mod read;
mod unique;
mod value;

use std::collections::BTreeSet;

pub use read::{Row, Table};
use serde::{Deserialize, Serialize};
pub(crate) use unique::Prior;
pub use value::{ColumnId, Value};

use super::Requirement;
use crate::encoding;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Operation {
    /// Creates the table, with these columns and their names.
    Create { columns: Vec<(ColumnId, String)> },
    /// Writes a row. What must be true of its key first is the rule.
    Put {
        key: Vec<u8>,
        cells: Vec<(ColumnId, Value)>,
        rule: Rule,
        /// The row this replaces, if its author read it.
        prior: Option<Prior>,
    },
    /// Removes a row, if there is one.
    Delete {
        key: Vec<u8>,
        /// The row, if its author read it.
        prior: Option<Prior>,
    },
    /// Adds a column, or renames one.
    NameColumn {
        column: ColumnId,
        name: String,
        /// Whether the column is new.
        add: bool,
    },
    /// Removes a column, if the table has it.
    DropColumn { column: ColumnId },
}

/// What a row's key must hold for a row to be written there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Rule {
    /// Nothing: the row is new.
    Insert,
    /// A row, which this one replaces.
    Replace,
    /// Either.
    Upsert,
}

/// What a key in a table's model is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// Holds a value while the table exists.
    Table,
    /// Holds a column's name while the table has that column.
    Column(ColumnId),
    /// Holds a value once a column has been added, for good. A dropped
    /// column can't be added again: rows may still hold what they were
    /// given for it, and that mustn't come back.
    Added(ColumnId),
    /// Holds a row's cells.
    Row(Vec<u8>),
    /// Holds the key of the row that has a value in a unique column.
    /// What follows the column is the value, encoded.
    Unique(ColumnId, Vec<u8>),
    /// Holds a value once the table has had a unique column, for good.
    Indexed,
}

const TABLE: u8 = 0;
const COLUMN: u8 = 1;
const ROW: u8 = 2;
const ADDED: u8 = 3;
const UNIQUE: u8 = 4;
const INDEXED: u8 = 5;

impl Operation {
    /// Whether a table takes the operation: every value is of its
    /// column's type, and no column is named twice.
    pub(crate) fn belongs(&self) -> bool {
        let mut seen = BTreeSet::new();
        match self {
            Self::Create { columns } => columns
                .iter()
                .all(|(column, _)| column.takes(&Value::Null) && seen.insert(column)),
            Self::Put {
                cells, rule, prior, ..
            } => {
                let typed = cells
                    .iter()
                    .all(|(column, value)| column.takes(value) && seen.insert(column));
                let read = match prior {
                    Some(prior) => *rule == Rule::Replace && prior.belongs(),
                    None => true,
                };

                typed && read
            }
            Self::NameColumn { column, .. } => column.takes(&Value::Null),
            Self::Delete { prior, .. } => prior.as_ref().is_none_or(Prior::belongs),
            Self::DropColumn { .. } => true,
        }
    }

    pub(crate) fn requires(&self) -> Vec<(Vec<u8>, Requirement)> {
        match self {
            Self::Create { .. } => vec![(Place::Table.key(), Requirement::Vacant)],
            Self::Put {
                key,
                cells,
                rule,
                prior,
            } => {
                let table = (Place::Table.key(), Requirement::Present);
                let columns = cells
                    .iter()
                    .map(|(column, _)| (Place::Column(*column).key(), Requirement::Present));
                let row = Place::Row(key.clone()).key();
                let row = match (prior, rule) {
                    (Some(prior), _) => vec![prior.requires(key)],
                    (None, Rule::Insert) => vec![(row, Requirement::Vacant)],
                    (None, Rule::Replace) => vec![(row, Requirement::Present), Self::unread()],
                    (None, Rule::Upsert) => vec![Self::unread()],
                };

                let unique = unique::requires(cells, prior.as_ref());
                let all = std::iter::once(table).chain(columns).chain(row);

                all.chain(unique).collect()
            }

            // Deleting a row that's already gone isn't a failure. Two
            // devices that delete the same row both got what they wanted.
            Self::Delete { key, prior } => {
                let row = match prior {
                    Some(prior) => prior.requires(key),
                    None => Self::unread(),
                };

                vec![(Place::Table.key(), Requirement::Present), row]
            }

            Self::DropColumn { .. } => vec![(Place::Table.key(), Requirement::Present)],

            Self::NameColumn { column, add, .. } => {
                let table = (Place::Table.key(), Requirement::Present);
                let column = if *add {
                    (Place::Added(*column).key(), Requirement::Vacant)
                } else {
                    (Place::Column(*column).key(), Requirement::Present)
                };

                vec![table, column]
            }
        }
    }

    /// Each key the operation writes, and what it holds afterwards.
    pub(crate) fn writes(&self) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
        match self {
            Self::Create { columns } => {
                let table = (Place::Table.key(), Some(Vec::new()));
                let columns = columns
                    .iter()
                    .flat_map(|(column, name)| Self::named(*column, name));

                std::iter::once(table).chain(columns).collect()
            }

            Self::Put {
                key, cells, prior, ..
            } => {
                let mut writes = unique::writes(key, cells, prior.as_ref());
                let cells: std::collections::BTreeMap<_, _> = cells.iter().cloned().collect();
                let row = encoding::encode(&cells);
                writes.push((Place::Row(key.clone()).key(), Some(row)));

                writes
            }

            Self::Delete { key, prior } => {
                let mut writes = unique::writes(key, &[], prior.as_ref());
                writes.push((Place::Row(key.clone()).key(), None));

                writes
            }

            Self::NameColumn { column, name, .. } => Self::named(*column, name),
            Self::DropColumn { column } => vec![(Place::Column(*column).key(), None)],
        }
    }
}

impl Operation {
    /// What a column with a name leaves behind.
    fn named(column: ColumnId, name: &str) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
        let name = name.to_owned().into_bytes();
        let indexed = column.is_unique().then(|| Place::Indexed.key());
        let places = [Place::Added(column).key()].into_iter().chain(indexed);
        let marks = places.map(|place| (place, Some(Vec::new())));

        marks
            .chain([(Place::Column(column).key(), Some(name))])
            .collect()
    }

    /// What a write to a row requires if its author didn't say what the
    /// row was: that the table never had a unique column.
    fn unread() -> (Vec<u8>, Requirement) {
        (Place::Indexed.key(), Requirement::Vacant)
    }
}

impl Place {
    pub(crate) fn key(&self) -> Vec<u8> {
        match self {
            Self::Table => vec![TABLE],
            Self::Column(column) => [&[COLUMN][..], column.as_bytes()].concat(),
            Self::Added(column) => [&[ADDED][..], column.as_bytes()].concat(),
            Self::Row(key) => [&[ROW][..], key].concat(),
            Self::Unique(column, value) => [&[UNIQUE][..], column.as_bytes(), value].concat(),
            Self::Indexed => vec![INDEXED],
        }
    }

    /// The key that holds whichever row has `value` in `column`.
    pub(crate) fn unique(column: ColumnId, value: &Value) -> Vec<u8> {
        Self::Unique(column, encoding::encode(value)).key()
    }

    pub(crate) fn parse(key: &[u8]) -> Option<Self> {
        let (&kind, rest) = key.split_first()?;
        match kind {
            TABLE if rest.is_empty() => Some(Self::Table),
            COLUMN => Some(Self::Column(ColumnId::from_bytes(rest.try_into().ok()?))),
            ROW => Some(Self::Row(rest.to_vec())),
            ADDED => Some(Self::Added(ColumnId::from_bytes(rest.try_into().ok()?))),
            UNIQUE => {
                let (column, value) = rest.split_first_chunk()?;

                Some(Self::Unique(ColumnId::from_bytes(*column), value.to_vec()))
            }
            INDEXED if rest.is_empty() => Some(Self::Indexed),
            _ => None,
        }
    }
}
