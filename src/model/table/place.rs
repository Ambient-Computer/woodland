//! The keys a table keeps in its model.

use super::ColumnId;
use crate::encoding;
use crate::model::Fingerprint;

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
    /// Holds the key of the row that has a value in a unique column.
    /// What follows the column is the value, encoded.
    Unique(ColumnId, Vec<u8>),
}

const TABLE: u8 = 0;
const COLUMN: u8 = 1;
const ROW: u8 = 2;
const ADDED: u8 = 3;
const UNIQUE: u8 = 4;

/// What follows a row's key, in the keys of its parts.
const KEYS: u8 = 1;
const CELL: u8 = 2;

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
            Self::Cell(key, incarnation, column) => {
                let row = escape(key);

                [&[ROW][..], &row, &[CELL], incarnation, column.as_bytes()].concat()
            }
            Self::Unique(column, value) => [&[UNIQUE][..], column.as_bytes(), value].concat(),
        }
    }

    /// The key that holds whichever row has `value` in `column`.
    pub(crate) fn unique(column: ColumnId, value: &super::Value) -> Vec<u8> {
        Self::Unique(column, encoding::encode(value)).key()
    }

    pub(crate) fn parse(key: &[u8]) -> Option<Self> {
        let (&kind, rest) = key.split_first()?;
        match kind {
            TABLE if rest.is_empty() => Some(Self::Table),
            COLUMN => Some(Self::Column(ColumnId::from_bytes(rest.try_into().ok()?))),
            ADDED => Some(Self::Added(ColumnId::from_bytes(rest.try_into().ok()?))),
            ROW => {
                let (row, part) = unescape(rest)?;
                match part.split_first() {
                    None => Some(Self::Row(row)),
                    Some((&KEYS, [])) => Some(Self::Keys(row)),
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
            _ => None,
        }
    }
}

/// The incarnation an insert has before its transaction is written.
pub(crate) const UNSEALED: Incarnation = [0; 16];

/// What the key of a row holds while this incarnation of it exists.
pub(crate) fn incarnate(incarnation: Incarnation) -> (Vec<u8>, Fingerprint) {
    (incarnation.to_vec(), Fingerprint::of(&incarnation))
}

/// Writes bytes so that they can be followed by more, and still sort
/// the way they did: a zero becomes a zero and `0xFF`, and two zeroes
/// end it. Nothing written this way is the start of anything else
/// written this way.
pub(crate) fn escape(bytes: &[u8]) -> Vec<u8> {
    let mut escaped = Vec::with_capacity(bytes.len() + 2);
    for &byte in bytes {
        escaped.push(byte);
        if byte == 0 {
            escaped.push(0xFF);
        }
    }

    escaped.extend([0, 0]);
    escaped
}

/// Reads back what [`escape`] wrote, and whatever follows it.
pub(crate) fn unescape(escaped: &[u8]) -> Option<(Vec<u8>, &[u8])> {
    let mut bytes = Vec::new();
    let mut rest = escaped;
    loop {
        match rest {
            [0, 0, after @ ..] => return Some((bytes, after)),
            [0, 0xFF, after @ ..] => {
                bytes.push(0);
                rest = after;
            }
            [0, ..] | [] => return None,
            [byte, after @ ..] => {
                bytes.push(*byte);
                rest = after;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaped_bytes_come_back() {
        for bytes in [&b""[..], b"ada", b"\0", b"a\0\0b", b"\0\xFF", b"\xFF\0"] {
            let mut escaped = escape(bytes);
            escaped.extend(b"more");

            assert_eq!(unescape(&escaped), Some((bytes.to_vec(), &b"more"[..])));
        }
    }

    #[test]
    fn escaped_bytes_sort_like_the_bytes() {
        let keys = [
            &b""[..],
            b"\0",
            b"\0\0",
            b"\0a",
            b"a",
            b"a\0",
            b"a\0b",
            b"ab",
        ];
        let escaped: Vec<_> = keys.iter().map(|key| escape(key)).collect();

        assert!(escaped.is_sorted());
    }

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
        let places = [
            Place::Table,
            Place::Column(column),
            Place::Added(column),
            Place::Row(b"a\0b".to_vec()),
            Place::Keys(b"".to_vec()),
            Place::Cell(b"ada".to_vec(), [9; 16], column),
            Place::Unique(column, b"value".to_vec()),
        ];

        for place in places {
            assert_eq!(Place::parse(&place.key()), Some(place));
        }
    }
}
