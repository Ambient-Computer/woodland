use crate::model::table::Place;
use crate::model::{ColumnId, ModelId};

use super::GroupId;

/// What a step changed in the models, for anyone watching.
///
/// This is how to subscribe to a [`Query`]: keep it, and run it again
/// when a change [affects] it. A change says where to look, not what's
/// there now, and it may be reported when nothing a reader sees is
/// different.
///
/// There are two kinds of read, and a change is to one of them:
///
/// - **Optimistic** reads change when a write arrives, or stops counting.
/// - **Committed** reads change when a write is committed. An optimistic read
///   of the same key already showed it, and doesn't change again.
///
/// [`Query`]: crate::Query
/// [affects]: crate::Query::is_affected_by
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Changed {
    /// One key of one model.
    Key {
        group: GroupId,
        model: ModelId,
        key: Vec<u8>,
        /// Whether it's committed reads that changed, or optimistic ones.
        committed: bool,
    },
    /// What's [pending] for a table: a change that waits to be
    /// committed was written, was committed, or stopped counting. No
    /// read is different because of it.
    ///
    /// [pending]: crate::Database::pending
    Pending { group: GroupId, model: ModelId },
    /// An [index] of a column has every row: it was just added, or
    /// has caught up after everything in the group changed. Reads
    /// through it may have missed rows until now.
    ///
    /// [index]: crate::Database::add_index
    Indexed {
        group: GroupId,
        model: ModelId,
        column: ColumnId,
    },
    /// Committed reads in the group are now of the checkpoint at this
    /// height, exactly: every key it changed has been reported, in this
    /// step or the ones before. No read is different because of it.
    Checkpoint { group: GroupId, height: u64 },
    /// Anything in the group, for either kind of read. The device
    /// caught up from a snapshot, or dropped writes that were discarded.
    Everything { group: GroupId },
}

impl Changed {
    /// The key of the [table] row a change is to, if it's to one: the
    /// row itself, or anything it holds. A copy of a table kept
    /// somewhere else reads that row [again].
    ///
    /// [table]: ModelId::table
    /// [again]: crate::Query::row
    #[must_use]
    pub fn row(&self) -> Option<Vec<u8>> {
        match self {
            Self::Key { key, .. } => Place::row_of(key),
            _ => None,
        }
    }
}
