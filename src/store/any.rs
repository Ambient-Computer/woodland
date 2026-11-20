//! One store that is any of the drivers.
//!
//! The core is compiled once for each kind of store a database is
//! opened on, and a second kind costs most of the core again. An
//! application that picks its driver while running opens its database
//! on this instead, and the core is compiled once.

#[cfg(all(feature = "browser", target_arch = "wasm32"))]
use super::{BrowserStore, BrowserView};
use super::{Entry, MemoryStore, MemoryView, Order, Store, StoreError, View, Write};
#[cfg(feature = "file")]
use super::{FileStore, FileView};

/// Whichever driver the application chose while running. It adds
/// nothing to the one it holds.
///
/// Make one from a driver with `into`. The library never chooses, and
/// never falls back from one driver to another:
///
/// ```
/// use woodland::{AnyStore, MemoryStore};
///
/// # let durable: Option<AnyStore> = None;
/// // `durable` is the device's own store, if it could be opened.
/// let store: AnyStore = match durable {
///     Some(store) => store,
///     None => MemoryStore::new().into(),
/// };
/// ```
///
/// Falling back to memory is the application's call, because of what
/// it means. A [`MemoryStore`] keeps nothing, so the device is a new
/// peer every time it starts, and whatever it signed is forgotten
/// with it. A role given to such a device is weight that vanishes
/// with the session: it's best left holding groups without one.
#[derive(Debug)]
#[non_exhaustive]
pub enum AnyStore {
    Memory(MemoryStore),
    #[cfg(feature = "file")]
    File(FileStore),
    #[cfg(all(feature = "browser", target_arch = "wasm32"))]
    Browser(BrowserStore),
}

/// An [`AnyStore`] at one moment.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum AnyView {
    Memory(MemoryView),
    #[cfg(feature = "file")]
    File(FileView),
    #[cfg(all(feature = "browser", target_arch = "wasm32"))]
    Browser(BrowserView),
}

impl AnyStore {
    /// Whether what's applied outlasts the process. A store that
    /// isn't durable makes the device a new peer each time it starts.
    #[must_use]
    pub fn is_durable(&self) -> bool {
        !matches!(self, Self::Memory(_))
    }
}

impl From<MemoryStore> for AnyStore {
    fn from(store: MemoryStore) -> Self {
        Self::Memory(store)
    }
}

#[cfg(feature = "file")]
impl From<FileStore> for AnyStore {
    fn from(store: FileStore) -> Self {
        Self::File(store)
    }
}

#[cfg(all(feature = "browser", target_arch = "wasm32"))]
impl From<BrowserStore> for AnyStore {
    fn from(store: BrowserStore) -> Self {
        Self::Browser(store)
    }
}

impl Store for AnyStore {
    type View = AnyView;

    async fn apply(&mut self, batch: Vec<Write>) -> Result<(), StoreError> {
        match self {
            Self::Memory(store) => store.apply(batch).await,
            #[cfg(feature = "file")]
            Self::File(store) => store.apply(batch).await,
            #[cfg(all(feature = "browser", target_arch = "wasm32"))]
            Self::Browser(store) => store.apply(batch).await,
        }
    }

    fn view(&self) -> AnyView {
        match self {
            Self::Memory(store) => AnyView::Memory(store.view()),
            #[cfg(feature = "file")]
            Self::File(store) => AnyView::File(store.view()),
            #[cfg(all(feature = "browser", target_arch = "wasm32"))]
            Self::Browser(store) => AnyView::Browser(store.view()),
        }
    }

    async fn unreturned(&self) -> Result<u64, StoreError> {
        match self {
            Self::Memory(store) => store.unreturned().await,
            #[cfg(feature = "file")]
            Self::File(store) => store.unreturned().await,
            #[cfg(all(feature = "browser", target_arch = "wasm32"))]
            Self::Browser(store) => store.unreturned().await,
        }
    }

    async fn reclaim(&mut self) -> Result<(), StoreError> {
        match self {
            Self::Memory(store) => store.reclaim().await,
            #[cfg(feature = "file")]
            Self::File(store) => store.reclaim().await,
            #[cfg(all(feature = "browser", target_arch = "wasm32"))]
            Self::Browser(store) => store.reclaim().await,
        }
    }
}

impl View for AnyView {
    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        match self {
            Self::Memory(view) => view.get(key).await,
            #[cfg(feature = "file")]
            Self::File(view) => view.get(key).await,
            #[cfg(all(feature = "browser", target_arch = "wasm32"))]
            Self::Browser(view) => view.get(key).await,
        }
    }

    async fn scan(
        &self,
        start: &[u8],
        end: Option<&[u8]>,
        order: Order,
        limit: usize,
    ) -> Result<Vec<Entry>, StoreError> {
        match self {
            Self::Memory(view) => view.scan(start, end, order, limit).await,
            #[cfg(feature = "file")]
            Self::File(view) => view.scan(start, end, order, limit).await,
            #[cfg(all(feature = "browser", target_arch = "wasm32"))]
            Self::Browser(view) => view.scan(start, end, order, limit).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::ready;

    use super::*;
    use crate::store::contract;
    use crate::testing::block_on;

    #[test]
    fn it_does_what_every_store_does_in_memory() {
        block_on(contract::holds(|| {
            ready(AnyStore::from(MemoryStore::new()))
        }));
    }

    #[test]
    fn memory_isnt_durable() {
        assert!(!AnyStore::from(MemoryStore::new()).is_durable());
    }

    #[cfg(feature = "file")]
    #[test]
    fn it_does_what_every_store_does_in_a_file() {
        let folder = tempfile::tempdir().expect("a folder");
        let made = std::cell::Cell::new(0);

        block_on(contract::holds(|| {
            made.set(made.get() + 1);
            let path = folder.path().join(made.get().to_string());
            let store = FileStore::open(path).expect("opened");
            let store = AnyStore::from(store);
            assert!(store.is_durable());

            ready(store)
        }));
    }
}
