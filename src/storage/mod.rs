//! What the library asks of storage: an ordered key-value store, and
//! nothing more.
//!
//! Every backend can do this well: `SQLite` (one table with a `BLOB` primary
//! key), `IndexedDB` (key ranges), or a `BTreeMap` in tests. The library never
//! asks for queries. Instead, it encodes its structure into keys so that
//! everything it needs to load together shares a prefix (see [`keys`]).
//!
//! What can grow without bound is never read in one go. The log comes
//! back a page at a time: by key to send or settle it, and as a bounded
//! range scan at startup.
//!
//! # The contract
//!
//! - **Writes are atomic.** Apply all of a [`StorageAction::Write`] or none.
//! - **Writes are durable before the next action.** Messages that follow a
//!   write may advertise what it stored. A crash between the two must never
//!   leave peers believing in data that was lost.
//! - **Reads answer later.** Perform a [`StorageAction::Scan`] or
//!   [`StorageAction::Get`] whenever convenient and report the result
//!   with [`Batch::scanned`] or [`Batch::got`]. The node never blocks
//!   waiting for storage.
//! - **Reads see earlier writes.** A read must reflect every write that
//!   came before it in the action list. A snapshot relies on it: the scan
//!   it's built from must hold everything the node had settled when it
//!   asked.
//!
//! [`Batch::scanned`]: crate::Batch::scanned
//! [`Batch::got`]: crate::Batch::got

pub(crate) mod keys;

/// How many transactions to read, send or settle at once. Small in
/// tests, so that they cross page boundaries all the time.
pub(crate) const PAGE: usize = if cfg!(test) { 3 } else { 256 };

/// Something for the caller to do with storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageAction {
    /// Apply these changes atomically, in order.
    Write(Vec<StorageWrite>),

    /// Read the entries whose keys are in `start..end`, in key order, and
    /// report them with [`Batch::scanned`]. With a `limit`, stop after
    /// that many: the node asks for the rest if it wants them.
    ///
    /// [`Batch::scanned`]: crate::Batch::scanned
    Scan {
        start: Vec<u8>,
        end: Vec<u8>,
        limit: Option<usize>,
    },

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
