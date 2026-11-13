//! Unique columns: no two rows hold the same value in one.
//!
//! Whether a column is unique is part of its ID, like its type, so an
//! operation knows what it has to require without reading anything.
//!
//! Each value a row holds in a unique column is taken, under a key of
//! its own ([`Place::Unique`]), the way a claim takes a name: writing
//! the row requires that key to be vacant, and of two rows that want
//! one value, the first in the timeline gets it and the other's
//! transaction is pruned.
//!
//! Changing or deleting the row has to give its values back, and an
//! operation can't look them up: it says what they were, and requires
//! the row to be exactly the one its author read ([`Prior`]). A write
//! that doesn't say can't be trusted to leave the values right, so in a
//! table that ever had a unique column, only inserts go without.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{ColumnId, Place, Value};
use crate::model::{Fingerprint, Requirement};

/// The row an operation replaces or deletes, as its author read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Prior {
    /// Stands for the row as it was stored.
    pub(crate) was: Fingerprint,
    /// What it held in unique columns.
    pub(crate) held: Vec<(ColumnId, Value)>,
}

type Written = (Vec<u8>, Option<Vec<u8>>);

impl Prior {
    /// Whether it could be what a row held: a value, of the column's
    /// type, for each of some unique columns.
    pub(crate) fn belongs(&self) -> bool {
        let mut seen = BTreeSet::new();
        let holds = |(column, value): &(ColumnId, Value)| {
            column.is_unique() && *value != Value::Null && column.takes(value)
        };

        self.held
            .iter()
            .all(|held| holds(held) && seen.insert(held.0))
    }

    pub(crate) fn requires(&self, row: &[u8]) -> (Vec<u8>, Requirement) {
        (Place::Row(row.to_vec()).key(), Requirement::Is(self.was))
    }
}

/// The values a row's cells take in unique columns.
pub(crate) fn taken(cells: &[(ColumnId, Value)]) -> impl Iterator<Item = &(ColumnId, Value)> {
    cells
        .iter()
        .filter(|(column, value)| column.is_unique() && *value != Value::Null)
}

/// What writing `cells` over `prior` requires of unique values: each
/// one the row didn't already hold must be free.
pub(crate) fn requires(
    cells: &[(ColumnId, Value)],
    prior: Option<&Prior>,
) -> Vec<(Vec<u8>, Requirement)> {
    let held = prior.map_or(&[][..], |prior| &prior.held);
    let new = taken(cells).filter(|cell| !held.contains(cell));

    new.map(|(column, value)| (Place::unique(*column, value), Requirement::Vacant))
        .collect()
}

/// What writing `cells` over `prior`, under the row key `row`, does to
/// unique values: gives back each one the row no longer holds, and
/// takes each one it didn't.
pub(crate) fn writes(
    row: &[u8],
    cells: &[(ColumnId, Value)],
    prior: Option<&Prior>,
) -> Vec<Written> {
    let held = prior.map_or(&[][..], |prior| &prior.held);
    let kept = |cell: &&(ColumnId, Value)| taken(cells).any(|it| it == *cell);
    let freed = held.iter().filter(|cell| !kept(cell));
    let freed = freed.map(|(column, value)| (Place::unique(*column, value), None));
    let new = taken(cells).filter(|cell| !held.contains(cell));
    let new = new.map(|(column, value)| (Place::unique(*column, value), Some(row.to_vec())));

    freed.chain(new).collect()
}
