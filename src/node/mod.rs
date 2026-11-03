mod batch;
mod network;

pub use batch::Batch;
use network::Network;

use crate::io::{Event, NetworkEvent};

/// Represents this device. Applications should only create one.
///
/// Manages current state, updates, and subscriptions.
#[derive(Debug, Default)]
pub struct Node {
    network: Network,
}

impl Node {
    #[must_use]
    pub fn new() -> Self {
        Self {
            network: Network::default(),
        }
    }

    /// Starts a batch of events to process together.
    pub fn batch(&mut self) -> Batch<'_> {
        Batch::new(self)
    }

    fn process(&mut self, events: Vec<Event>) {
        for event in events {
            match event {
                Event::Network(NetworkEvent::Connected { peer }) => {
                    self.network.connect(peer);
                }

                Event::Network(NetworkEvent::Disconnected { peer }) => {
                    self.network.disconnect(&peer);
                }

                Event::Network(NetworkEvent::Message { .. }) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{SigningKey, VerifyingKey};

    use super::*;

    fn key(seed: u8) -> VerifyingKey {
        SigningKey::from_bytes(&[seed; 32]).verifying_key()
    }

    #[test]
    fn tracks_connected_peers() {
        let mut node = Node::new();
        node.batch().connected(key(1)).connected(key(2)).process();

        assert!(node.network.is_connected(&key(1).into()));
        assert!(node.network.is_connected(&key(2).into()));
    }

    #[test]
    fn forgets_disconnected_peers() {
        let mut node = Node::new();
        node.batch()
            .connected(key(1))
            .disconnected(key(1))
            .process();

        assert!(!node.network.is_connected(&key(1).into()));
    }

    #[test]
    fn ignores_disconnects_from_unknown_peers() {
        let mut node = Node::new();
        node.batch()
            .connected(key(1))
            .disconnected(key(2))
            .process();

        assert!(node.network.is_connected(&key(1).into()));
    }
}
