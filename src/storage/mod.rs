//! What the library asks of storage: an ordered key-value store, and
//! nothing more.
//!
//! Every backend can do this well: `SQLite` (one table with a `BLOB` primary
//! key), `IndexedDB` (key ranges), or a `BTreeMap` in tests. The library never
//! asks for queries. Instead, it encodes its structure into keys so that
//! everything it needs to load together shares a prefix (see [`keys`]).
//!
//! # The contract
//!
//! - **Writes are atomic.** Apply all of a [`StorageAction::Write`] or none.
//! - **Writes are durable before the next action.** Messages that follow a
//!   write may advertise what it stored. A crash between the two must never
//!   leave peers believing in data that was lost.
//! - **Reads answer later.** Perform a [`StorageAction::Scan`] or
//!   [`StorageAction::Get`] whenever convenient and report the result with
//!   [`Batch::scanned`] or [`Batch::got`]. The node never blocks waiting
//!   for storage.
//! - **Reads see earlier writes.** A read must reflect every write that
//!   came before it in the action list. The node relies on it to keep its
//!   cache of model entries in step with storage.
//!
//! [`Batch::scanned`]: crate::Batch::scanned
//! [`Batch::got`]: crate::Batch::got

pub(crate) mod keys;

/// Something for the caller to do with storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageAction {
    /// Apply these changes atomically, in order.
    Write(Vec<StorageWrite>),

    /// Read every entry whose key starts with `prefix`, in key order, and
    /// report them with [`Batch::scanned`].
    ///
    /// [`Batch::scanned`]: crate::Batch::scanned
    Scan { prefix: Vec<u8> },

    /// Read each of these keys, and report what's stored under each one
    /// (or that nothing is) with [`Batch::got`].
    ///
    /// [`Batch::got`]: crate::Batch::got
    Get { keys: Vec<Vec<u8>> },
}

/// One change within an atomic write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageWrite {
    Put {
        key: Vec<u8>,
        value: Vec<u8>,
    },
    Delete {
        key: Vec<u8>,
    },
    /// Deletes every key in `start..end`.
    DeleteRange {
        start: Vec<u8>,
        end: Vec<u8>,
    },
}
