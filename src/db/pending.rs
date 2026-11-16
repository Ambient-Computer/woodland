//! Changes that are waiting to be committed before they do anything.

use ed25519_dalek::VerifyingKey;

use crate::model::table::{Deferred, Each};
use crate::model::{ColumnId, Filter, ModelId};

/// A change that's been written and hasn't taken effect, because what
/// it does is decided when its transaction is committed. Until then
/// no read shows it.
///
/// It may never take effect: its transaction can still be dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    /// The table it's for.
    pub model: ModelId,
    /// The device that wrote it.
    pub author: VerifyingKey,
    pub change: Waiting,
}

/// What a [`Pending`] change will do.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Waiting {
    /// A row gets the next number of a column's sequence.
    Number { row: Vec<u8>, column: ColumnId },
    /// Every row a filter picks gets new values.
    Update { filter: Filter },
    /// Every row a filter picks is deleted.
    DeleteWhere { filter: Filter },
    /// The model is dropped, with everything in it.
    Drop,
    /// A column is removed from a table.
    DropColumn { column: ColumnId },
    /// Every row has to be one a filter picks, from then on.
    Rule { filter: Filter },
    /// A row is deleted, along with the rows that refer to it or only
    /// if there are none.
    Delete { row: Vec<u8>, with_referrers: bool },
}

impl From<&Deferred> for Waiting {
    fn from(deferred: &Deferred) -> Self {
        match deferred {
            Deferred::Number { row, column, .. } => Self::Number {
                row: row.clone(),
                column: *column,
            },

            Deferred::Each { filter, change } => {
                let filter = filter.clone();
                match change {
                    Each::Set(_) => Self::Update { filter },
                    Each::Delete => Self::DeleteWhere { filter },
                }
            }

            Deferred::DropColumn { column } => Self::DropColumn { column: *column },
            Deferred::Rule { filter } => Self::Rule {
                filter: filter.clone(),
            },

            Deferred::Remove { row, cascade, .. } => Self::Delete {
                row: row.clone(),
                with_referrers: *cascade,
            },
        }
    }
}
