//! Reading a query a part at a time.
//!
//! The last of a query's ranges is the one that's read in parts.
//! Any before it are what's needed to make sense of the rest, a
//! table's columns, and come whole with every part.
//!
//! A part ends between two things, never inside one. In a table a
//! thing is a row, with all its cells. Anywhere else it's a key.

use super::{Query, Range};
use crate::db::keys::{self, Record};
use crate::model::Kind;
use crate::model::table::{self, Place};

impl Query {
    /// The query for what comes whole with every part, if anything
    /// does.
    pub(crate) fn fixed(&self) -> Option<Self> {
        let (_, fixed) = self.ranges.split_last()?;
        let mut query = self.clone();
        query.ranges = fixed.to_vec();

        (!fixed.is_empty()).then_some(query)
    }

    /// The first key of what's read in parts.
    pub(crate) fn first(&self) -> Vec<u8> {
        self.parted()
            .map(|(start, _)| start.clone())
            .unwrap_or_default()
    }

    /// Where storage keeps the keys from `from` up to `to` of what's
    /// read in parts, as [`Query::storage_ranges`] would give it. No
    /// `to` is as far as the query goes.
    pub(crate) fn span(&self, from: &[u8], to: Option<&[u8]>) -> Vec<(Vec<u8>, Vec<u8>)> {
        let Some((_, end)) = self.parted() else {
            return Vec::new();
        };

        let to = match (to, end.as_deref()) {
            (Some(to), Some(end)) => Some(to.min(end)),
            (to, end) => to.or(end),
        };

        let mut query = self.clone();
        query.ranges = vec![(from.to_vec(), to.map(<[u8]>::to_vec))];
        query.storage_ranges()
    }

    /// Whether `key` is past everything the query asks for.
    pub(crate) fn is_past(&self, key: &[u8]) -> bool {
        let end = self.parted().and_then(|(_, end)| end.as_deref());

        end.is_some_and(|end| end <= key)
    }

    /// The model's key that a key in storage is for.
    pub(crate) fn place(stored: &[u8]) -> Option<Vec<u8>> {
        match keys::parse(stored)? {
            Record::Committed { key, .. }
            | Record::Version { key, .. }
            | Record::Index { key, .. } => Some(key),
        }
    }

    /// The first key after the thing `key` is part of: after the row,
    /// if it's part of one, and after the key if not.
    pub(crate) fn past(&self, key: &[u8]) -> Vec<u8> {
        let row = self.is_table().then(|| Place::row_of(key)).flatten();
        let past = row.and_then(|row| table::successor(&Place::Row(row).key()));

        past.unwrap_or_else(|| keys::after(key))
    }

    /// Whether a key that was found is one of the things a limit
    /// counts: a row, or a row's place under a value, and not one of a
    /// row's cells or a column's name. Outside a table every key is.
    pub(crate) fn counts(&self, key: &[u8]) -> bool {
        let counted = || {
            let place = Place::parse(key);

            matches!(
                place,
                Some(Place::Row(_) | Place::Unique(..) | Place::Index(..))
            )
        };

        !self.is_table() || counted()
    }

    /// The same query, for what's at `from` and after.
    pub(crate) fn rest(&self, from: Vec<u8>) -> Self {
        let mut query = self.clone();
        if let Some((start, _)) = query.ranges.last_mut() {
            *start = from;
        }

        query
    }

    fn parted(&self) -> Option<&Range> {
        self.ranges.last()
    }

    fn is_table(&self) -> bool {
        self.model.kind() == Some(Kind::Table)
    }
}
