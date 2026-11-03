//! Describes a batch of events, both local and remote, processed together.
//! Processing returns a set of actions for the caller to perform.

mod event;

pub(crate) use event::{Event, NetworkEvent};
