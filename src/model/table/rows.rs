//! What changing rows requires and writes.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::place::{self, Incarnation, Place};
use super::{ColumnId, Value};
use crate::encoding;
use crate::model::{Fingerprint, Requirement};

type Cells = Vec<(ColumnId, Value)>;
type Required = Vec<(Vec<u8>, Requirement)>;
type Written = Vec<(Vec<u8>, Option<Vec<u8>>)>;

/// The row an operation changes: its key, and which incarnation of it
/// the author read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Target {
    pub(crate) key: Vec<u8>,
    pub(crate) incarnation: Incarnation,
}

/// A change to what a row holds in unique columns: everything it held
/// as its author read it, and everything it holds afterwards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Rekey {
    pub(crate) from: Cells,
    pub(crate) to: Cells,
}

impl Target {
    /// The row is still the one that was read.
    fn requires(&self) -> (Vec<u8>, Requirement) {
        let (_, incarnation) = place::incarnate(self.incarnation);

        (
            Place::Row(self.key.clone()).key(),
            Requirement::Is(incarnation),
        )
    }

    fn cell(&self, column: ColumnId) -> Vec<u8> {
        Place::Cell(self.key.clone(), self.incarnation, column).key()
    }
}

/// Whether these could be a row's cells: each of its column's type, and
/// no column twice.
pub(crate) fn typed(cells: &[(ColumnId, Value)]) -> bool {
    let mut seen = BTreeSet::new();

    cells
        .iter()
        .all(|(column, value)| column.takes(value) && seen.insert(column))
}

/// Whether these could be everything a row holds in unique columns.
pub(crate) fn keyed(cells: &[(ColumnId, Value)]) -> bool {
    let unique = |(column, value): &(ColumnId, Value)| column.is_unique() && *value != Value::Null;

    typed(cells) && cells.iter().all(unique)
}

/// Whether these could be cells of columns that aren't unique.
pub(crate) fn plain(cells: &[(ColumnId, Value)]) -> bool {
    typed(cells) && cells.iter().all(|(column, _)| !column.is_unique())
}

/// The cells that go under the row's unique values.
fn keys(cells: &[(ColumnId, Value)]) -> Cells {
    let unique = |(column, value): &&(ColumnId, Value)| column.is_unique() && *value != Value::Null;

    cells.iter().filter(unique).cloned().collect()
}

/// What the key of a row's unique values holds: nothing, if it has
/// none.
fn stored(keys: &[(ColumnId, Value)]) -> Option<Vec<u8>> {
    let keys: BTreeMap<_, _> = keys.iter().cloned().collect();

    (!keys.is_empty()).then(|| encoding::encode(&keys))
}

/// The row's unique values are still what was read.
fn unchanged(row: &[u8], keys: &[(ColumnId, Value)]) -> (Vec<u8>, Requirement) {
    let requirement = match stored(keys) {
        Some(stored) => Requirement::Is(Fingerprint::of(&stored)),
        None => Requirement::Vacant,
    };

    (Place::Keys(row.to_vec()).key(), requirement)
}

/// What going from one set of unique values to another requires: each
/// value the row didn't hold must be free, and its column there.
fn rekey_requires(from: &[(ColumnId, Value)], to: &[(ColumnId, Value)]) -> Required {
    let taken = to.iter().filter(|cell| !from.contains(cell));
    let taken = taken.flat_map(|(column, value)| {
        [
            (Place::Column(*column).key(), Requirement::Present),
            (Place::unique(*column, value), Requirement::Vacant),
        ]
    });

    taken.collect()
}

/// What going from one set of unique values to another writes: gives
/// back each value the row no longer holds, takes each it didn't.
fn rekey_writes(row: &[u8], from: &[(ColumnId, Value)], to: &[(ColumnId, Value)]) -> Written {
    let freed = from.iter().filter(|cell| !to.contains(cell));
    let freed = freed.map(|(column, value)| (Place::unique(*column, value), None));
    let taken = to.iter().filter(|cell| !from.contains(cell));
    let taken = taken.map(|(column, value)| (Place::unique(*column, value), Some(row.to_vec())));
    let held = (stored(from) != stored(to)).then(|| (Place::Keys(row.to_vec()).key(), stored(to)));

    freed.chain(taken).chain(held).collect()
}

fn cell_writes(target: &Target, cells: &[(ColumnId, Value)]) -> Written {
    let plain = cells.iter().filter(|(column, _)| !column.is_unique());
    let write = |(column, value): &(ColumnId, Value)| {
        let value = (*value != Value::Null).then(|| encoding::encode(value));

        (target.cell(*column), value)
    };

    plain.map(write).collect()
}

pub(crate) fn insert_requires(key: &[u8], cells: &[(ColumnId, Value)]) -> Required {
    let table = (Place::Table.key(), Requirement::Present);
    let row = (Place::Row(key.to_vec()).key(), Requirement::Vacant);
    let columns = cells
        .iter()
        .map(|(column, _)| (Place::Column(*column).key(), Requirement::Present));
    let unique = keys(cells);
    let unique = unique
        .iter()
        .map(|(column, value)| (Place::unique(*column, value), Requirement::Vacant));

    [table, row]
        .into_iter()
        .chain(columns)
        .chain(unique)
        .collect()
}

pub(crate) fn insert_writes(target: &Target, cells: &[(ColumnId, Value)]) -> Written {
    let (incarnation, _) = place::incarnate(target.incarnation);
    let row = (Place::Row(target.key.clone()).key(), Some(incarnation));
    let live: Cells = cells
        .iter()
        .filter(|(_, value)| *value != Value::Null)
        .cloned()
        .collect();

    let mut writes = vec![row];
    writes.extend(rekey_writes(&target.key, &[], &keys(cells)));
    writes.extend(cell_writes(target, &live));
    writes
}

pub(crate) fn set_requires(
    target: &Target,
    cells: &[(ColumnId, Value)],
    rekey: Option<&Rekey>,
) -> Required {
    let columns = cells
        .iter()
        .map(|(column, _)| (Place::Column(*column).key(), Requirement::Present));
    let mut requires: Required = std::iter::once(target.requires()).chain(columns).collect();
    if let Some(rekey) = rekey {
        requires.push(unchanged(&target.key, &rekey.from));
        requires.extend(rekey_requires(&rekey.from, &rekey.to));
    }

    requires
}

pub(crate) fn set_writes(
    target: &Target,
    cells: &[(ColumnId, Value)],
    rekey: Option<&Rekey>,
) -> Written {
    let mut writes = cell_writes(target, cells);
    if let Some(rekey) = rekey {
        writes.extend(rekey_writes(&target.key, &rekey.from, &rekey.to));
    }

    writes
}

pub(crate) fn delete_requires(target: &Target, keys: &[(ColumnId, Value)]) -> Required {
    vec![target.requires(), unchanged(&target.key, keys)]
}

pub(crate) fn delete_writes(
    target: &Target,
    cells: &[ColumnId],
    keys: &[(ColumnId, Value)],
) -> Written {
    let mut writes = rekey_writes(&target.key, keys, &[]);
    writes.extend(cells.iter().map(|column| (target.cell(*column), None)));
    writes.push((Place::Row(target.key.clone()).key(), None));
    writes
}
