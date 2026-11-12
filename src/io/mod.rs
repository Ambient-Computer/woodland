//! Describes a batch of events, both local and remote, processed together.
//! Processing returns a set of actions for the caller to perform.

mod action;
mod event;

pub use action::{Action, Changed, NetworkAction};
pub(crate) use event::{Event, LocalEvent, NetworkEvent, StorageEvent};
