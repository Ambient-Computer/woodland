//! The keys a table keeps in its model.

use std::ops::Bound;

use super::order::{self, escape, unescape};
use super::{ColumnId, Value};
use crate::model::Fingerprint;
use crate::model::query::Range;

/// Tells one row that had a key from the next one that has it. See
/// the module's notes on rows.
pub(crate) type Incarnation = [u8; 16];

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
    /// Holds a row's incarnation while the row exists.
    Row(Vec<u8>),
    /// Holds everything a row has in unique columns, as one value.
    Keys(Vec<u8>),
    /// Holds what one incarnation of a row has in one column.
    Cell(Vec<u8>, Incarnation, ColumnId),
    /// Holds a value once something has referred to a row, until the
    /// row is deleted.
    Referred(Vec<u8>),
    /// Holds a token while a row is retired: it takes no more
    /// references.
    Retired(Vec<u8>),
    /// Holds the key of the row that has a value in a unique column.
    /// What follows the column is the value, in a form that sorts the
    /// way values do (see [`order`]).
    Unique(ColumnId, Vec<u8>),
    /// Holds a value while a row has a value in an indexed column: the
    /// column, the value as in [`Place::Unique`], and the row's key.
    Index(ColumnId, Vec<u8>, Vec<u8>),
    /// Holds the last number given out in a column that numbers its
    /// rows.
    Sequence(ColumnId),
    /// Holds a condition every row has to meet, while it's in force.
    /// What follows is a hash of the condition.
    Rule([u8; 32]),
}

const TABLE: u8 = 0;
const COLUMN: u8 = 1;
const ROW: u8 = 2;
const ADDED: u8 = 3;
const UNIQUE: u8 = 4;
const INDEX: u8 = 5;
const SEQUENCE: u8 = 6;
const RULE: u8 = 7;

/// What follows a row's key, in the keys of its parts.
const KEYS: u8 = 1;
const CELL: u8 = 2;
const REFERRED: u8 = 3;
const RETIRED: u8 = 4;

impl Place {
    /// Everything about a row sorts together, and rows sort by key: the
    /// row's key comes first, written so that no key is the start of
    /// another (see [`escape`]).
    pub(crate) fn key(&self) -> Vec<u8> {
        match self {
            Self::Table => vec![TABLE],
            Self::Column(column) => [&[COLUMN][..], column.as_bytes()].concat(),
            Self::Added(column) => [&[ADDED][..], column.as_bytes()].concat(),
            Self::Row(key) => [&[ROW][..], &escape(key)].concat(),
            Self::Keys(key) => [&[ROW][..], &escape(key), &[KEYS]].concat(),
            Self::Referred(key) => [&[ROW][..], &escape(key), &[REFERRED]].concat(),
            Self::Retired(key) => [&[ROW][..], &escape(key), &[RETIRED]].concat(),
            Self::Cell(key, incarnation, column) => {
                let row = escape(key);

                [&[ROW][..], &row, &[CELL], incarnation, column.as_bytes()].concat()
            }
            Self::Unique(column, value) => [&[UNIQUE][..], column.as_bytes(), value].concat(),
            Self::Index(column, value, row) => {
                [&[INDEX][..], column.as_bytes(), value, row].concat()
            }
            Self::Sequence(column) => [&[SEQUENCE][..], column.as_bytes()].concat(),
            Self::Rule(rule) => [&[RULE][..], rule].concat(),
        }
    }

    /// The key that holds whichever row has `value` in `column`.
    pub(crate) fn unique(column: ColumnId, value: &Value) -> Vec<u8> {
        Self::Unique(column, order::ordered(value)).key()
    }

    /// The key that says the row under `row` has `value` in `column`.
    pub(crate) fn index(column: ColumnId, value: &Value, row: &[u8]) -> Vec<u8> {
        Self::Index(column, order::ordered(value), row.to_vec()).key()
    }

    /// Where the values of a column are looked up: every key that
    /// starts with this is for one of them, in the order of the
    /// values. `None` if the column is neither unique nor indexed.
    pub(crate) fn lookup(column: ColumnId) -> Option<Vec<u8>> {
        let kind = match (column.is_unique(), column.is_indexed()) {
            (true, _) => UNIQUE,
            (false, true) => INDEX,
            (false, false) => return None,
        };

        Some([&[kind][..], column.as_bytes()].concat())
    }

    /// Where the rows are, and where they end.
    pub(crate) fn rows() -> (Vec<u8>, Vec<u8>) {
        (vec![ROW], vec![ROW + 1])
    }

    pub(crate) fn parse(key: &[u8]) -> Option<Self> {
        let (&kind, rest) = key.split_first()?;
        match kind {
            TABLE if rest.is_empty() => Some(Self::Table),
            COLUMN => Some(Self::Column(ColumnId::from_bytes(rest.try_into().ok()?))),
            ADDED => Some(Self::Added(ColumnId::from_bytes(rest.try_into().ok()?))),
            SEQUENCE => Some(Self::Sequence(ColumnId::from_bytes(rest.try_into().ok()?))),
            RULE => Some(Self::Rule(rest.try_into().ok()?)),
            ROW => {
                let (row, part) = unescape(rest)?;
                match part.split_first() {
                    None => Some(Self::Row(row)),
                    Some((&KEYS, [])) => Some(Self::Keys(row)),
                    Some((&REFERRED, [])) => Some(Self::Referred(row)),
                    Some((&RETIRED, [])) => Some(Self::Retired(row)),
                    Some((&CELL, cell)) => {
                        let (incarnation, column) = cell.split_first_chunk()?;
                        let column = ColumnId::from_bytes(column.try_into().ok()?);

                        Some(Self::Cell(row, *incarnation, column))
                    }
                    Some(_) => None,
                }
            }
            UNIQUE => {
                let (column, value) = rest.split_first_chunk()?;

                Some(Self::Unique(ColumnId::from_bytes(*column), value.to_vec()))
            }
            INDEX => {
                let (column, entry) = rest.split_first_chunk()?;
                let column = ColumnId::from_bytes(*column);
                let row = order::after(column, entry)?;
                let value = &entry[..entry.len() - row.len()];

                Some(Self::Index(column, value.to_vec(), row.to_vec()))
            }
            _ => None,
        }
    }
}

/// Where everything a read of the whole table needs is: that it
/// exists, its columns, and its rows.
pub(crate) fn whole() -> (Vec<u8>, Vec<u8>) {
    (vec![TABLE], vec![ROW + 1])
}

/// Where the table's rules are.
pub(crate) fn rules() -> (Vec<u8>, Vec<u8>) {
    (vec![RULE], vec![RULE + 1])
}

/// Where the table says it exists and what its columns are: what any
/// read of its rows needs to make sense of them.
pub(crate) fn schema() -> Range {
    (vec![TABLE], Some(vec![COLUMN + 1]))
}

/// Where the rows with keys between two bounds are. Everything about a
/// row starts with its key, and no row's key starts another's, so
/// it's one range.
pub(crate) fn rows(start: Bound<Vec<u8>>, end: Bound<Vec<u8>>) -> Range {
    let (first, last) = Place::rows();
    let row = |key: &[u8]| Place::Row(key.to_vec()).key();
    let past = |key: &[u8]| order::successor(&row(key)).unwrap_or(last.clone());
    let start = match start {
        Bound::Included(key) => row(&key),
        Bound::Excluded(key) => past(&key),
        Bound::Unbounded => first,
    };

    let end = match end {
        Bound::Included(key) => past(&key),
        Bound::Excluded(key) => row(&key),
        Bound::Unbounded => last.clone(),
    };

    (start, Some(end))
}

/// Where it says which rows hold a value between two bounds in a
/// column. `None` if the column is neither unique nor indexed.
pub(crate) fn values(column: ColumnId, start: Bound<&Value>, end: Bound<&Value>) -> Option<Range> {
    let first = Place::lookup(column)?;
    let last = order::successor(&first)?;
    let at = |value: &Value| [&first[..], &order::ordered(value)].concat();
    let past = |value: &Value| order::successor(&at(value)).unwrap_or(last.clone());
    let start = match start {
        Bound::Included(value) => at(value),
        Bound::Excluded(value) => past(value),
        Bound::Unbounded => first.clone(),
    };

    let end = match end {
        Bound::Included(value) => past(value),
        Bound::Excluded(value) => at(value),
        Bound::Unbounded => last.clone(),
    };

    Some((start, Some(end)))
}

/// The incarnation an insert has before its transaction is written.
pub(crate) const UNSEALED: Incarnation = [0; 16];

/// What the key of a row holds while this incarnation of it exists.
pub(crate) fn incarnate(incarnation: Incarnation) -> (Vec<u8>, Fingerprint) {
    (incarnation.to_vec(), Fingerprint::of(&incarnation))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_key_is_never_part_of_another_rows() {
        let incarnation = [7; 16];
        let column = ColumnId::from_bytes([1; 8]);
        let short = Place::Row(b"a".to_vec()).key();
        for longer in [&b"ab"[..], b"a\0", b"a\x01", b"a\x02"] {
            let other = Place::Cell(longer.to_vec(), incarnation, column).key();

            assert!(!other.starts_with(&short));
        }
    }

    #[test]
    fn every_place_parses() {
        let column = ColumnId::from_bytes([3; 8]);
        let indexed = ColumnId::text(crate::model::ModelId::from_bytes([0; 64]), [1; 32]).indexed();
        let places = [
            Place::Table,
            Place::Column(column),
            Place::Added(column),
            Place::Row(b"a\0b".to_vec()),
            Place::Keys(b"".to_vec()),
            Place::Referred(b"ada".to_vec()),
            Place::Retired(b"ada".to_vec()),
            Place::Cell(b"ada".to_vec(), [9; 16], column),
            Place::Unique(column, b"value".to_vec()),
            Place::Sequence(column),
            Place::Rule([4; 32]),
            Place::Index(indexed, order::ordered(&"a\0b".into()), b"row".to_vec()),
        ];

        for place in places {
            assert_eq!(Place::parse(&place.key()), Some(place));
        }
    }
}
