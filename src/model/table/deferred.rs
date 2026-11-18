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
use std::ops::Bound;

use serde::{Deserialize, Serialize};

use super::place::{self, Incarnation, Place};
use super::rows::{self, Rekey, Target};
use super::value::Type;
use super::{ColumnId, Filter, Table, Value};
use crate::encoding;
use crate::model::{Kind, ModelId, Reads, Requirement, Resolved};
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
    /// Removes a column, if the table has it. What's written to the
    /// column before this in the timeline stands, and nothing after.
    DropColumn { column: ColumnId },
    /// Puts a rule in force: from here on, every row has to be one
    /// the filter picks.
    Rule { filter: Filter },
    /// Deletes a row that may have been referred to, once every
    /// reference there will ever be can be found.
    Remove {
        row: Vec<u8>,
        incarnation: Incarnation,
        /// Where what refers to the row is: each of these columns, in
        /// a table of the same clock, holds the key of the row its own
        /// row refers to.
        referrers: Vec<(ModelId, ColumnId)>,
        /// Whether rows that refer to it go with it. If not, there
        /// have to be none.
        cascade: bool,
    },
}

/// What's done to each row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Each {
    /// Gives these columns new values.
    Set(Vec<(ColumnId, Value)>),
    Delete,
}

impl Deferred {
    /// The blobs it writes references to, in whichever rows it turns
    /// out to change.
    pub(crate) fn blobs(&self) -> Vec<crate::blob::BlobRef> {
        match self {
            Self::Each {
                change: Each::Set(cells),
                ..
            } => super::blobs_in(cells),
            _ => Vec::new(),
        }
    }

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

            Self::DropColumn { .. } => true,
            Self::Rule { filter } => filter.typed(),
            Self::Remove { referrers, .. } => referrers.iter().all(|(table, column)| {
                let names = matches!(column.kind(), Some(Type::Text | Type::Bytes));

                names && table.kind() == Some(Kind::Table)
            }),
        }
    }

    /// Some of what it comes to, given what's committed before it:
    /// steps to take in order, each checked against what the ones
    /// before it left, and where to carry on from. `None` if it can't
    /// be done.
    ///
    /// It reads about `limit` keys, so that one that takes in a whole
    /// table is worked out a part at a time. `state` is what the steps
    /// found so far have left: none of them changes a row that's still
    /// to be read.
    pub(crate) async fn resolve(
        &self,
        state: &impl Reads,
        at: &Cursor,
        limit: usize,
    ) -> Result<Option<Found>, StoreError> {
        let once = |steps: Vec<Resolved>| Found { steps, next: None };
        match self {
            Self::Number {
                row,
                incarnation,
                column,
            } => {
                let numbered = number(row, *incarnation, *column, state).await?;

                Ok(numbered.map(|it| once(vec![it])))
            }

            Self::Each { filter, change } => each(filter, change, state, at, limit).await,
            Self::DropColumn { column } => Ok(Some(once(vec![Resolved {
                model: state.model(),
                requires: vec![(Place::Table.key(), Requirement::Present)],
                writes: vec![(Place::Column(*column).key(), None)],
            }]))),
            Self::Rule { filter } => rule(filter, state, at, limit).await,
            Self::Remove {
                row,
                incarnation,
                referrers,
                cascade,
            } => remove(row, *incarnation, referrers, *cascade, state, at, limit).await,
        }
    }
}

/// How far working one out has got.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Cursor {
    /// Which of the things it goes through, for one that goes through
    /// several.
    stage: usize,
    /// The row to carry on from.
    from: Option<Vec<u8>>,
}

/// Some of what a change that waited comes to.
#[derive(Debug)]
pub(crate) struct Found {
    pub(crate) steps: Vec<Resolved>,
    /// Where the rest is worked out from, if there's more.
    pub(crate) next: Option<Cursor>,
}

/// Some of a table's rows, from the row `from` on, each of them whole,
/// and the row to carry on from if there are more. `None` if there's
/// no table.
async fn some_rows(
    state: &impl Reads,
    from: Option<&Vec<u8>>,
    limit: usize,
) -> Result<Option<(Table, Option<Vec<u8>>)>, StoreError> {
    let (start, end) = place::schema();
    let schema = state.scan(&start, &end.unwrap_or_default()).await?;
    if Table::read(&schema).is_none() {
        return Ok(None);
    }

    let mut limit = limit.max(1);
    loop {
        let first = from.map_or(Bound::Unbounded, |row| Bound::Included(row.clone()));
        let (start, end) = place::rows(first, Bound::Unbounded);
        let page = state.page(&start, &end.unwrap_or_default(), limit).await?;

        // The row the page stops in may have more to it. It waits for
        // the next page, unless it's where this one began: then it's
        // too big for a page, and the page has to grow.
        let cut = page.reach.as_deref().and_then(Place::row_of);
        if cut.is_some() && cut.as_ref() == from {
            limit = limit.saturating_mul(2);
            continue;
        }

        let whole = |(key, _): &(Vec<u8>, Vec<u8>)| cut.is_none() || Place::row_of(key) != cut;
        let mut read = schema;
        read.extend(page.found.into_iter().filter(whole));
        let next = cut.filter(|_| page.reach.is_some());

        return Ok(Table::read(&read).map(|table| (table, next)));
    }
}

/// A rule the rows already break can't be put in force. Mending them
/// first is up to whoever wants it.
async fn rule(
    filter: &Filter,
    state: &impl Reads,
    at: &Cursor,
    limit: usize,
) -> Result<Option<Found>, StoreError> {
    let Some((table, next)) = some_rows(state, at.from.as_ref(), limit).await? else {
        return Ok(None);
    };
    if !table.rows().iter().all(|row| filter.picks(row)) {
        return Ok(None);
    }
    if let Some(from) = next {
        let next = Some(Cursor {
            stage: 0,
            from: Some(from),
        });

        return Ok(Some(Found {
            steps: Vec::new(),
            next,
        }));
    }

    let kept = (
        Place::Rule(filter.id()).key(),
        Some(encoding::encode(filter)),
    );

    Ok(Some(Found {
        steps: vec![Resolved {
            model: state.model(),
            requires: Vec::new(),
            writes: vec![kept],
        }],
        next: None,
    }))
}

/// Whether a cell names the row under `key`.
fn names(value: &Value, key: &[u8]) -> bool {
    match value {
        Value::Text(text) => text.as_bytes() == key,
        Value::Bytes(bytes) => bytes == key,
        _ => false,
    }
}

/// Finds every row that names this one in a column it was told to
/// look in, a column at a time, and deletes them and then the row, or
/// gives up if there are any.
///
/// Nothing can be placed before this any more, so the rows found are
/// all there will ever be. A reference placed after it is pruned,
/// because the row is gone. That's what the mark on a referred row and
/// retiring it were standing in for, and here neither is asked about.
///
/// A row that goes with it is deleted the ordinary way: if something
/// has referred to that row in turn, none of this happens.
async fn remove(
    key: &[u8],
    incarnation: Incarnation,
    referrers: &[(ModelId, ColumnId)],
    cascade: bool,
    state: &impl Reads,
    at: &Cursor,
    limit: usize,
) -> Result<Option<Found>, StoreError> {
    let Some(&(model, column)) = referrers.get(at.stage) else {
        let bound = || Bound::Included(key.to_vec());
        let (start, end) = place::schema();
        let mut read = state.scan(&start, &end.unwrap_or_default()).await?;
        let (start, end) = place::rows(bound(), bound());
        read.extend(state.scan(&start, &end.unwrap_or_default()).await?);
        let own = Table::read(&read);
        let found = own.as_ref().and_then(|table| table.row(key));
        let Some(row) = found.filter(|row| row.incarnation() == incarnation) else {
            return Ok(None);
        };

        let (target, keys) = (row.target(), row.keys());
        let cells: Vec<_> = row.stored().collect();

        return Ok(Some(Found {
            steps: vec![Resolved {
                model: state.model(),
                requires: vec![rows::target(&target), rows::unchanged(key, keys)],
                writes: rows::delete_writes(&target, &cells, keys, true),
            }],
            next: None,
        }));
    };

    let Some(other) = state.of(model) else {
        return Ok(None);
    };
    let onward = Cursor {
        stage: at.stage + 1,
        from: None,
    };
    let Some((table, next)) = some_rows(&other, at.from.as_ref(), limit).await? else {
        return Ok(Some(Found {
            steps: Vec::new(),
            next: Some(onward),
        }));
    };

    // A row found once is gone from what's read after, so one that
    // names this row in two columns is still deleted once.
    let mut steps = Vec::new();
    for referrer in table.rows() {
        let itself = model == state.model() && referrer.key() == key;
        if itself || !names(referrer.get(column), key) {
            continue;
        }
        if !cascade {
            return Ok(None);
        }

        let operation = referrer.delete();
        steps.push(Resolved {
            model,
            requires: operation.requires(),
            writes: operation.writes(),
        });
    }

    let same = |from| Cursor {
        stage: at.stage,
        from: Some(from),
    };

    Ok(Some(Found {
        steps,
        next: Some(next.map_or(onward, same)),
    }))
}

/// Reads the table a part at a time, and changes each row the filter
/// picks the way a transaction that had just read it would. So
/// everything a change to a row requires is required here, row by
/// row: a value two rows are both given in a unique column, or a row
/// something refers to, stops all of it.
async fn each(
    filter: &Filter,
    change: &Each,
    state: &impl Reads,
    at: &Cursor,
    limit: usize,
) -> Result<Option<Found>, StoreError> {
    let Some((table, next)) = some_rows(state, at.from.as_ref(), limit).await? else {
        return Ok(None);
    };

    let picked = table.rows().iter().filter(|row| filter.picks(row));
    let changed = picked.map(|row| {
        let operation = match change {
            Each::Set(cells) => row.set(cells.iter().cloned()),
            Each::Delete => row.delete(),
        };

        Resolved {
            model: state.model(),
            requires: operation.requires(),
            writes: operation.writes(),
        }
    });
    let steps = changed.collect();
    let next = next.map(|from| Cursor {
        stage: 0,
        from: Some(from),
    });

    Ok(Some(Found { steps, next }))
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
    let (cells, rekey) = if column.is_unique() {
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
        model: state.model(),
        requires: rows::set_requires(&target, &cells, rekey.as_ref()),
        writes,
    }))
}
