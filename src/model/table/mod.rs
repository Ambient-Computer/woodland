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
//! - **Present** is how a transaction depends on another. Inserting a
//!   row requires its table, and every column it has a value for. If
//!   the transaction that created the table was pruned, then so is
//!   every transaction that wrote to it, however many there were and
//!   whoever wrote them.
//!
//! So a device can create a table and fill it while offline, and none
//! of it is final until it settles. Until then it either all stands, or
//! goes together.
//!
//! # What's stored
//!
//! Three kinds of key (see [`Place`]): one that says the table exists,
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
mod value;

use std::collections::BTreeSet;

pub use read::{Row, Table};
use serde::{Deserialize, Serialize};
pub use value::{ColumnId, Value};

use super::Requirement;
use crate::encoding;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Operation {
    /// Creates the table, with these columns and their names.
    Create { columns: Vec<(ColumnId, String)> },
    /// Adds a row.
    Insert {
        key: Vec<u8>,
        cells: Vec<(ColumnId, Value)>,
    },
}

/// What a key in a table's model is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// Holds a value while the table exists.
    Table,
    /// Holds a column's name while the table has that column.
    Column(ColumnId),
    /// Holds a row's cells.
    Row(Vec<u8>),
}

const TABLE: u8 = 0;
const COLUMN: u8 = 1;
const ROW: u8 = 2;

impl Operation {
    /// Whether a table takes the operation: every value is of its
    /// column's type, and no column is named twice.
    pub(crate) fn belongs(&self) -> bool {
        let mut seen = BTreeSet::new();
        match self {
            Self::Create { columns } => columns
                .iter()
                .all(|(column, _)| column.takes(&Value::Null) && seen.insert(column)),
            Self::Insert { cells, .. } => cells
                .iter()
                .all(|(column, value)| column.takes(value) && seen.insert(column)),
        }
    }

    pub(crate) fn requires(&self) -> Vec<(Vec<u8>, Requirement)> {
        match self {
            Self::Create { .. } => vec![(Place::Table.key(), Requirement::Vacant)],
            Self::Insert { key, cells } => {
                let table = (Place::Table.key(), Requirement::Present);
                let columns = cells
                    .iter()
                    .map(|(column, _)| (Place::Column(*column).key(), Requirement::Present));
                let row = (Place::Row(key.clone()).key(), Requirement::Vacant);

                std::iter::once(table).chain(columns).chain([row]).collect()
            }
        }
    }

    /// Each key the operation writes, and what it holds afterwards.
    pub(crate) fn writes(&self) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
        match self {
            Self::Create { columns } => {
                let table = (Place::Table.key(), Some(Vec::new()));
                let columns = columns.iter().map(|(column, name)| {
                    let name = name.clone().into_bytes();

                    (Place::Column(*column).key(), Some(name))
                });

                std::iter::once(table).chain(columns).collect()
            }

            Self::Insert { key, cells } => {
                let cells: std::collections::BTreeMap<_, _> = cells.iter().cloned().collect();
                let row = encoding::encode(&cells);

                vec![(Place::Row(key.clone()).key(), Some(row))]
            }
        }
    }
}

impl Place {
    pub(crate) fn key(&self) -> Vec<u8> {
        match self {
            Self::Table => vec![TABLE],
            Self::Column(column) => [&[COLUMN][..], column.as_bytes()].concat(),
            Self::Row(key) => [&[ROW][..], key].concat(),
        }
    }

    pub(crate) fn parse(key: &[u8]) -> Option<Self> {
        let (&kind, rest) = key.split_first()?;
        match kind {
            TABLE if rest.is_empty() => Some(Self::Table),
            COLUMN => Some(Self::Column(ColumnId::from_bytes(rest.try_into().ok()?))),
            ROW => Some(Self::Row(rest.to_vec())),
            _ => None,
        }
    }
}
