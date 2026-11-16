//! The store: where the library keeps its data.
//!
//! An ordered map from bytes to bytes, with atomic batches and consistent
//! views. Nothing richer is asked for, so that the same contract can be
//! met by a file on a server and by a browser's database, and so that
//! everything built on top behaves the same on both.
//!
//! Stores are the library's own: one driver per place it runs, each
//! behind a feature, all held to this contract by the same tests. An
//! application chooses one. It doesn't write one.
//!
//! # The contract
//!
//! - **Batches are atomic.** All of a batch is applied, or none of it,
//!   whenever the process dies.
//! - **Batches are durable once applied.** When [`Store::apply`]
//!   finishes, the batch survives a crash. The library tells no peer
//!   anything until then, because a device that forgets what it said can
//!   break agreement.
//! - **Views are consistent.** A [`View`] sees every batch applied before
//!   it was taken, and none applied after. It's held briefly: a store may
//!   make batches wait for it.
//! - **Calls are awaited.** Some stores answer at once and some can't.
//!   The library waits for nothing else, so with a store that answers at
//!   once it never waits at all.
//! - **One owner.** Nothing else writes to a store while the library has
//!   it, and a driver refuses a second owner where it can tell.
//!
//! # Losing a store
//!
//! A device that loses its store has forgotten promises it signed, and
//! must not carry on under the same key. So the key is kept in the store,
//! and they're lost together: the device comes back as someone new. A
//! store restored from a backup is no different. It has forgotten
//! everything since the copy was taken.

#[cfg(any(test, all(feature = "browser", target_arch = "wasm32")))]
mod browser;
#[cfg(test)]
mod contract;
#[cfg(feature = "file")]
mod file;
mod memory;

use std::error::Error;
use std::fmt;

#[cfg(all(feature = "browser", target_arch = "wasm32"))]
pub use browser::{BrowserStore, BrowserView};
#[cfg(feature = "file")]
pub use file::{FileStore, FileView};
pub use memory::{MemoryStore, MemoryView};

/// One change within a batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Write {
    Put { key: Vec<u8>, value: Vec<u8> },
    Delete { key: Vec<u8> },
}

/// A key and what's under it.
pub type Entry = (Vec<u8>, Vec<u8>);

/// Which way to walk a range of keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Ascending,
    Descending,
}

/// The store couldn't do what was asked: the disk is full, the file is
/// gone, the browser took the storage back.
///
/// Nothing can be done about it here. The step that hit it changes
/// nothing and says nothing, and the application decides what's next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreError {
    reason: String,
}

impl StoreError {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "the store failed: {}", self.reason)
    }
}

impl Error for StoreError {}

/// Somewhere to keep data. What it promises is in `docs/spec/storage.md`.
///
/// Its futures aren't required to be `Send`. A store in a browser can't
/// be, and the library never needs to move one between threads.
pub trait Store {
    type View: View;

    /// Applies every change in `batch`, in order, or none of them.
    ///
    /// # Errors
    ///
    /// If the batch couldn't be made durable. None of it was applied.
    fn apply(&mut self, batch: Vec<Write>) -> impl Future<Output = Result<(), StoreError>>;

    /// The store as it is now, to read from.
    fn view(&self) -> Self::View;
}

/// A store at one moment.
pub trait View: Clone {
    /// What's under `key`, if anything.
    ///
    /// # Errors
    ///
    /// If the store can't be read.
    fn get(&self, key: &[u8]) -> impl Future<Output = Result<Option<Vec<u8>>, StoreError>>;

    /// Up to `limit` entries with keys in `start..end`, in `order`. With
    /// no `end`, the range runs to the last key.
    ///
    /// A range is read a page at a time: ascending, the next page starts
    /// just after the last key returned, and descending, it ends at it.
    ///
    /// # Errors
    ///
    /// If the store can't be read.
    fn scan(
        &self,
        start: &[u8],
        end: Option<&[u8]>,
        order: Order,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<Entry>, StoreError>>;
}
