mod batch;
mod network;

pub use batch::Batch;
use network::Network;

use crate::io::{Action, Event, NetworkEvent};

/// Represents this device. Applications should only create one.
///
/// Manages current state, updates, and subscriptions.
///
/// A state machine that never performs I/O itself. Each [`Batch`] of
/// events moves it to a new state, and the [`Action`]s it returns are how
/// that state reaches the outside world. Performing them eventually
/// produces new events (a reply arrives, a peer drops), which go into the
/// next batch.
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

    fn process(&mut self, events: Vec<Event>) -> Vec<Action> {
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

        Vec::new()
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
        let actions = node.batch().connected(key(1)).connected(key(2)).process();
        assert_eq!(actions, []);

        assert!(node.network.is_connected(&key(1).into()));
        assert!(node.network.is_connected(&key(2).into()));
    }

    #[test]
    fn forgets_disconnected_peers() {
        let mut node = Node::new();
        let actions = node
            .batch()
            .connected(key(1))
            .disconnected(key(1))
            .process();
        assert_eq!(actions, []);

        assert!(!node.network.is_connected(&key(1).into()));
    }

    #[test]
    fn ignores_disconnects_from_unknown_peers() {
        let mut node = Node::new();
        let actions = node
            .batch()
            .connected(key(1))
            .disconnected(key(2))
            .process();
        assert_eq!(actions, []);

        assert!(node.network.is_connected(&key(1).into()));
    }
}
