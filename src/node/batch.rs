use ed25519_dalek::VerifyingKey;

use super::Node;
use crate::io::{Action, Event, NetworkEvent};

/// Events to process together. Start one with [`Node::batch`].
#[derive(Debug)]
#[must_use = "nothing happens until the batch is processed"]
pub struct Batch<'a> {
    node: &'a mut Node,
    events: Vec<Event>,
}

impl<'a> Batch<'a> {
    pub(super) fn new(node: &'a mut Node) -> Self {
        Self {
            node,
            events: Vec::new(),
        }
    }

    /// A connection to `peer` opened.
    pub fn connected(self, peer: VerifyingKey) -> Self {
        self.network(NetworkEvent::Connected { peer: peer.into() })
    }

    /// The connection to `peer` closed.
    pub fn disconnected(self, peer: VerifyingKey) -> Self {
        self.network(NetworkEvent::Disconnected { peer: peer.into() })
    }

    /// `peer` sent a message.
    pub fn message(self, peer: VerifyingKey) -> Self {
        self.network(NetworkEvent::Message { peer: peer.into() })
    }

    /// Processes every event, in the order they were added. Returns what
    /// the caller should do next.
    #[must_use = "actions must be performed"]
    pub fn process(self) -> Vec<Action> {
        self.node.process(self.events)
    }

    fn network(mut self, event: NetworkEvent) -> Self {
        self.events.push(Event::Network(event));
        self
    }
}
