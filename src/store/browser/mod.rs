//! The store for a browser: `IndexedDB`.
//!
//! `IndexedDB` is an ordered map with atomic transactions, which is most
//! of the contract already. What it lacks is a view that lasts, and
//! [`history`] makes up for that.

mod history;
#[cfg(all(feature = "browser", target_arch = "wasm32"))]
mod indexed;
#[cfg(all(feature = "browser", target_arch = "wasm32"))]
mod wait;

#[cfg(all(feature = "browser", target_arch = "wasm32"))]
pub use indexed::{BrowserStore, BrowserView};
