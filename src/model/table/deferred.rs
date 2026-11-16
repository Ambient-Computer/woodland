//! Changes to a table that are worked out when they're committed.
//!
//! Everything else a transaction does to a table is decided by its
//! author and written down, so every device applies the same thing
//! without reading a value. These can't be: what they do depends on
//! what the table holds at their place in the timeline, and no device
//! knows that until nothing can land before it. So they wait for a
//! checkpoint, and until then do nothing at all, in either kind of
//! read.
//!
//! Working one out means reading committed state and coming back with
//! ordinary requirements and writes. If it can't be done, or a
//! requirement doesn't hold, the whole transaction is pruned, for
//! good.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::place::{self, Incarnation, Place};
use super::rows::{self, Rekey, Target};
use super::value::Type;
use super::{ColumnId, Filter, Table, Value};
use crate::encoding;
use crate::model::{Reads, Resolved};
use crate::store::StoreError;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Deferred {
    /// Gives a row the next number of a column's sequence: one more
    /// than the last row to be given one, starting from 1. No number
    /// is skipped, because none is given out until it's final.
    Number {
        row: Vec<u8>,
        /// Which incarnation of the row its author read. `None` for a
        /// row its own transaction inserts.
        incarnation: Option<Incarnation>,
        column: ColumnId,
    },
    /// Does the same thing to every row a filter picks, of the rows
    /// there are once this is committed.
    Each { filter: Filter, change: Each },
}

/// What's done to each row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Each {
    /// Gives these columns new values.
    Set(Vec<(ColumnId, Value)>),
    Delete,
}

impl Deferred {
    /// Whether a table takes it: only whole numbers count rows.
    pub(crate) fn belongs(&self) -> bool {
        match self {
            Self::Number { column, .. } => column.kind() == Some(Type::Integer),
            Self::Each { filter, change } => {
                let typed = match change {
                    Each::Set(cells) => rows::typed(cells),
                    Each::Delete => true,
                };

                filter.typed() && typed
            }
        }
    }

    /// What it comes to, given what's committed before it: steps to
    /// take in order, each checked against what the ones before it
    /// left. `None` if it can't be done.
    pub(crate) async fn resolve(
        &self,
        state: &impl Reads,
    ) -> Result<Option<Vec<Resolved>>, StoreError> {
        match self {
            Self::Number {
                row,
                incarnation,
                column,
            } => {
                let numbered = number(row, *incarnation, *column, state).await?;

                Ok(numbered.map(|it| vec![it]))
            }

            Self::Each { filter, change } => each(filter, change, state).await,
        }
    }
}

/// Reads the whole table, and changes each row the filter picks the
/// way a transaction that had just read it would. So everything a
/// change to a row requires is required here, row by row: a value two
/// rows are both given in a unique column, or a row something refers
/// to, stops all of it.
async fn each(
    filter: &Filter,
    change: &Each,
    state: &impl Reads,
) -> Result<Option<Vec<Resolved>>, StoreError> {
    let (start, end) = place::whole();
    let Some(table) = Table::read(&state.scan(&start, &end).await?) else {
        return Ok(None);
    };

    let picked = table.rows().iter().filter(|row| filter.picks(row));
    let changed = picked.map(|row| {
        let operation = match change {
            Each::Set(cells) => row.set(cells.iter().cloned()),
            Each::Delete => row.delete(),
        };

        Resolved {
            requires: operation.requires(),
            writes: operation.writes(),
        }
    });

    Ok(Some(changed.collect()))
}

async fn number(
    row: &[u8],
    read: Option<Incarnation>,
    column: ColumnId,
    state: &impl Reads,
) -> Result<Option<Resolved>, StoreError> {
    let held = state.get(&Place::Row(row.to_vec()).key()).await?;
    let Some(incarnation) = held.and_then(|it| Incarnation::try_from(it).ok()) else {
        return Ok(None);
    };
    if read.is_some_and(|read| read != incarnation) {
        return Ok(None);
    }

    let sequence = Place::Sequence(column).key();
    let last = match state.get(&sequence).await? {
        Some(stored) => stored.try_into().ok().map(u64::from_be_bytes),
        None => Some(0),
    };
    let next = last.and_then(|last| last.checked_add(1));
    let Some((next, value)) = next.and_then(|next| Some((next, i64::try_from(next).ok()?))) else {
        return Ok(None);
    };

    // A row is numbered once. Numbering it again would leave the
    // number it had unused.
    let target = Target {
        key: row.to_vec(),
        incarnation,
    };
    let value = Value::Integer(value);
    let (cells, rekey) = if column.is_keyed() {
        let stored = state.get(&Place::Keys(row.to_vec()).key()).await?;
        let stored = stored.map(|it| encoding::decode::<BTreeMap<ColumnId, Value>>(&it));
        let Some(from) = stored.unwrap_or_else(|| Some(BTreeMap::new())) else {
            return Ok(None);
        };
        if from.contains_key(&column) {
            return Ok(None);
        }

        let from: Vec<_> = from.into_iter().collect();
        let to = from.iter().cloned().chain([(column, value)]).collect();

        (Vec::new(), Some(Rekey { from, to }))
    } else {
        let cell = Place::Cell(row.to_vec(), incarnation, column).key();
        if state.get(&cell).await?.is_some() {
            return Ok(None);
        }

        (vec![(column, value)], None)
    };

    let mut writes = rows::set_writes(&target, &cells, rekey.as_ref());
    writes.push((sequence, Some(next.to_be_bytes().to_vec())));

    Ok(Some(Resolved {
        requires: rows::set_requires(&target, &cells, rekey.as_ref()),
        writes,
    }))
}
