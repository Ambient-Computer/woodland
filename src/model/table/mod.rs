//! Tables: rows of typed cells, under a schema that's part of the data.
//!
//! A table is a model like any other. Everything about it is in the
//! timeline: creating it, its columns, and its rows are all written by
//! operations, in transactions, and the rules that keep it coherent are
//! the requirements every model has (see [`Requirement`]):
//!
//! - **Vacant** is how only one transaction gets something. Creating a
//!   table requires that it doesn't exist. Inserting a row requires that
//!   no row has its key. Of two transactions that try, the first in the
//!   timeline happens and the other is pruned, on every device.
//! - **Present** is how a transaction depends on another. Writing a
//!   cell requires its column, and inserting a row requires its table.
//!   If the transaction that created the table was pruned, then so is
//!   every transaction that wrote to it, however many there were and
//!   whoever wrote them.
//! - **Is** is how a transaction depends on what its author read.
//!   Changing a row requires it to be the row that was read, and not
//!   another that has since taken its key.
//!
//! So a device can create a table and fill it while offline, and none
//! of it is final until it is committed. Until then it either all stands, or
//! goes together.
//!
//! # Rows
//!
//! A row is several keys: one that says it exists, and one for each
//! cell. Changing a column writes that cell and nothing else. Two
//! devices that change different columns of a row both do, and of two
//! that change the same column, the later in the timeline is what's
//! read. Nothing is merged when a transaction is applied, because
//! nothing has to be: the columns never shared a value.
//!
//! A change says which row it's for, and a row's key isn't enough. A
//! row can be deleted and another inserted under its key, and a change
//! written for the first, placed between the two in the timeline, must
//! not show up in the second. So each insert makes a new **incarnation**
//! of its key: an ID nothing else has, worked out from the transaction
//! when it's written (see [`Operation::seal`]). Cells are kept under
//! it, and every change requires the row to still hold it. A change
//! for an incarnation that's gone is pruned. One that was placed just
//! before it went leaves a cell nothing reads.
//!
//! That's why a row has to be read before it's changed or deleted, and
//! why there's no "write this row, whatever is there": the incarnation
//! is what was there.
//!
//! # Unique columns
//!
//! A column can be unique: no two rows hold the same value in it.
//! Whether it is is part of its ID, like its type, so an operation
//! knows what it has to require without reading anything.
//!
//! Each value a row holds in a unique column is taken, under a key of
//! its own ([`Place::Unique`]), the way a claim takes a name: writing
//! it requires that key to be vacant, and of two rows that want one
//! value, the first in the timeline gets it.
//!
//! Changing or deleting the row has to give its values back, and an
//! operation can't look them up. So everything a row holds in unique
//! columns is one value ([`Place::Keys`]), and an operation that
//! changes any of it, or deletes the row, says what it was and
//! requires it to be exactly that. Unlike the other columns, then,
//! two devices that change a row's unique columns at once can't both
//! happen.
//!
//! # Indexes
//!
//! A column can be indexed instead: any number of rows can hold a
//! value, and each is found under the column, the value and its own
//! key ([`Place::Index`]). Values are written so that they sort the
//! way they compare (see [`order`]), and a range of them is a range of
//! keys. A unique column is found the same way.
//!
//! An index has to take back what a row held, like a unique column
//! does, so indexed columns are kept with the unique ones and follow
//! their rule: changes to them go by what was read, and exclude each
//! other. Being found without reading the table is paid for in merges.
//!
//! Rows are a range of keys too. A row's key is written so that no
//! key starts another (see [`order::escape`]), everything about a row
//! starts with it, and rows sort by it.
//!
//! # References
//!
//! A transaction can refer to a row: it depends on the row being
//! there, and is pruned if it isn't. That's half of what a foreign key
//! promises. The other half is that the row isn't deleted from under
//! what refers to it, and a delete written on a device that hasn't
//! heard of a reference can't know to stop.
//!
//! So a reference leaves a mark on the row, and a row with the mark
//! can't simply be deleted. A delete and a reference written apart
//! can't both happen: whichever comes second in the timeline is
//! pruned, the reference because the row is gone, or the delete
//! because the mark is there.
//!
//! A row that has been referred to is deleted in two steps. First
//! it's **retired**: it takes no more references, from there on in
//! the timeline. Once that is committed, nothing can be placed before
//! it, so every reference there will ever be is committed too, and a
//! committed read finds all of them. Then whoever is deleting the row
//! deals with what refers to it, and deletes it, in one transaction.
//! Whether that means deleting those rows too, or giving up and
//! restoring the row, is the application's choice. A delete names the
//! retirement it read, so one written before the row was restored
//! doesn't happen after.
//!
//! Waiting for the retirement to be committed is the price: it takes a
//! quorum. A rule about what doesn't exist anywhere can't be
//! checked by a device on its own.
//!
//! # Schema
//!
//! Columns are added, renamed and dropped by operations, like rows are
//! written. Each column is its own key, so two devices that each add a
//! column both succeed, and a write that names a column depends on
//! exactly that column: one written after the column was dropped, in
//! the timeline, is pruned.
//!
//! Adding and renaming take nothing away, and show at once. Dropping
//! waits to be committed (see [`Deferred`]): until then the column is
//! there for everyone, and from then on for no one. Taking something
//! away and then giving it back, because the transaction that took it
//! didn't happen after all, is worse than waiting.
//!
//! A row written before a column was dropped keeps what it was given
//! for it, and readers leave that out. Nothing goes back to rewrite
//! rows. A dropped column can't be added again, so that never
//! resurfaces.
//!
//! # Types
//!
//! A column's type is part of its ID (see [`ColumnId`]), so a cell is
//! checked against its column without reading anything. An operation
//! with a value of the wrong type isn't one a table takes, and its
//! transaction is pruned like any other that doesn't belong.
//!
//! # What waits
//!
//! Operations name the rows they change. "Every row where" depends on
//! the table's state at the transaction's place in the timeline, which
//! is a scan, and nothing that decides whether an uncommitted
//! transaction happens reads more than keys. So a change like that
//! isn't decided until it's committed: see [`Deferred`]. Whatever can
//! name its rows should, by reading them first, through an index or
//! by key, and not wait.
//!
//! # Rules
//!
//! A rule is a condition every row has to meet. Putting one in force
//! waits to be committed, because whether the rows there are meet it
//! isn't known before. From then on it's kept with the table, and a
//! transaction that would leave a row breaking it doesn't happen.
//!
//! That's decided when the transaction is committed, and not before:
//! what a row holds is the work of every transaction that wrote one of
//! its cells, and only the committed order says which. So an
//! optimistic read can show a row that a rule is about to turn down.
//!
//! Deleting a row that's been referred to can wait instead of going
//! through retirement: committed, it can read every row that refers
//! to the one it deletes, and take them with it or stop.

mod deferred;
mod filter;
mod order;
mod place;
mod read;
mod rows;
mod value;

pub(crate) use deferred::{Cursor, Deferred, Each};
pub use filter::Filter;
pub(crate) use order::successor;
use place::Incarnation;
pub(crate) use place::{Place, UNSEALED, rows, rules, schema, values};
pub use read::{Row, Table};
pub(crate) use rows::{Rekey, Target};
use serde::{Deserialize, Serialize};
pub use value::{ColumnId, Value};

use super::{Fingerprint, Requirement};
use crate::actor::Peer;
use crate::encoding;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Operation {
    /// Creates the table, with these columns and their names.
    Create { columns: Vec<(ColumnId, String)> },
    /// Adds a row. Its incarnation is filled in when the transaction
    /// is written.
    Insert {
        row: Target,
        cells: Vec<(ColumnId, Value)>,
    },
    /// Changes some of a row's cells.
    Set {
        row: Target,
        /// The new values of columns that aren't unique.
        cells: Vec<(ColumnId, Value)>,
        /// The change to its unique columns, if there is one.
        rekey: Option<Rekey>,
    },
    /// Removes a row.
    Delete {
        row: Target,
        /// The columns it has cells in, as its author read it.
        cells: Vec<ColumnId>,
        /// What it holds in unique and indexed columns, as its author
        /// read it.
        keys: Vec<(ColumnId, Value)>,
        /// The retirement it's in, as its author read it.
        retired: Option<Fingerprint>,
    },
    /// Depends on a row, and lets whoever deletes it know.
    Refer { row: Target },
    /// Stops a row taking references. The token is filled in when the
    /// transaction is written.
    Retire { row: Target, token: Incarnation },
    /// Lets a retired row take references again.
    Restore { row: Target },
    /// Adds a column, or renames one.
    NameColumn {
        column: ColumnId,
        name: String,
        /// Whether the column is new.
        add: bool,
    },
    /// Stops enforcing a rule. Putting one in force waits to be
    /// committed (see [`Deferred`]); letting go of one doesn't have to.
    Lift { rule: [u8; 32] },
}

impl Operation {
    /// Whether a table takes the operation: every value is of its
    /// column's type, and no column is named twice.
    pub(crate) fn belongs(&self) -> bool {
        match self {
            Self::Create { columns } => {
                let cells: Vec<_> = columns.iter().map(|(id, _)| (*id, Value::Null)).collect();

                rows::typed(&cells)
            }
            Self::Insert { cells, .. } => rows::typed(cells),
            Self::Set { cells, rekey, .. } => {
                let keyed = |rekey: &Rekey| rows::keyed(&rekey.from) && rows::keyed(&rekey.to);

                rows::plain(cells) && rekey.as_ref().is_none_or(keyed)
            }
            Self::Delete { keys, .. } => rows::keyed(keys),
            Self::NameColumn { column, .. } => column.takes(&Value::Null),
            Self::Lift { .. } | Self::Refer { .. } | Self::Retire { .. } | Self::Restore { .. } => {
                true
            }
        }
    }

    /// Gives an insert the incarnation of its row, and a retirement
    /// its token: one that no other has, because no other is this
    /// operation of this transaction. It's the author who works it
    /// out, once, like everything else a transaction can't be left to
    /// decide.
    pub(crate) fn seal(&mut self, author: Peer, seq: u64, index: usize) {
        let unique = match self {
            Self::Insert { row, .. } => &mut row.incarnation,
            Self::Retire { token, .. } => token,
            _ => return,
        };

        let hash = encoding::hash("woodland/row", &(author, seq, index));
        unique.copy_from_slice(&hash[..16]);
    }

    pub(crate) fn requires(&self) -> Vec<(Vec<u8>, Requirement)> {
        match self {
            Self::Create { .. } => vec![(Place::Table.key(), Requirement::Vacant)],
            Self::Insert { row, cells } => rows::insert_requires(&row.key, cells),
            Self::Set { row, cells, rekey } => rows::set_requires(row, cells, rekey.as_ref()),
            Self::Delete {
                row, keys, retired, ..
            } => rows::delete_requires(row, keys, *retired),
            Self::Refer { row } | Self::Retire { row, .. } => rows::refer_requires(row),
            Self::Restore { row } => rows::restore_requires(row),
            Self::Lift { rule } => vec![(Place::Rule(*rule).key(), Requirement::Present)],
            Self::NameColumn { column, add, .. } => {
                let table = (Place::Table.key(), Requirement::Present);
                let column = if *add {
                    (Place::Added(*column).key(), Requirement::Vacant)
                } else {
                    (Place::Column(*column).key(), Requirement::Present)
                };

                vec![table, column]
            }
        }
    }

    /// Each key the operation writes, and what it holds afterwards.
    pub(crate) fn writes(&self) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
        match self {
            Self::Create { columns } => {
                let table = (Place::Table.key(), Some(Vec::new()));
                let columns = columns
                    .iter()
                    .flat_map(|(column, name)| Self::named(*column, name));

                std::iter::once(table).chain(columns).collect()
            }

            Self::Insert { row, cells } => rows::insert_writes(row, cells),
            Self::Set { row, cells, rekey } => rows::set_writes(row, cells, rekey.as_ref()),
            Self::Delete {
                row,
                cells,
                keys,
                retired,
            } => rows::delete_writes(row, cells, keys, retired.is_some()),
            Self::Refer { row } => rows::refer_writes(row),
            Self::Retire { row, token } => rows::retire_writes(row, *token),
            Self::Restore { row } => rows::restore_writes(row),
            Self::NameColumn { column, name, .. } => Self::named(*column, name).into(),
            Self::Lift { rule } => vec![(Place::Rule(*rule).key(), None)],
        }
    }

    /// What a column with a name leaves behind.
    fn named(column: ColumnId, name: &str) -> [(Vec<u8>, Option<Vec<u8>>); 2] {
        let name = name.to_owned().into_bytes();

        [
            (Place::Added(column).key(), Some(Vec::new())),
            (Place::Column(column).key(), Some(name)),
        ]
    }
}
