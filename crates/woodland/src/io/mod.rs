//! Describes a batch of events, both local and remote, processed together.
//! Processing returns a set of actions for the caller to perform.

mod event;
mod network;

pub(crate) use event::{Event, NetworkEvent};
pub(crate) use network::Network;
