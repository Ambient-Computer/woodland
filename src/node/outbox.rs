use super::Network;
use crate::io::{Action, NetworkAction};

/// Collects what domains produce while processing a batch, then turns it
/// into actions once the whole batch is done.
///
/// Domains say what changed, not who needs it. Deciding who receives what
/// happens here, so each peer gets at most one message per batch.
#[derive(Debug, Default)]
pub(crate) struct Outbox {
    updates: Vec<Update>,
}

/// Something to share with interested peers. Contents come later.
#[derive(Debug)]
pub(crate) struct Update;

impl Outbox {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used once domains share updates")
    )]
    pub(crate) fn share(&mut self, update: Update) {
        self.updates.push(update);
    }

    /// Sends every connected peer one message, if there's anything to share.
    pub(crate) fn into_actions(self, network: &Network) -> Vec<Action> {
        if self.updates.is_empty() {
            return Vec::new();
        }

        network
            .connected()
            .map(|peer| Action::Network(NetworkAction::Send { peer: peer.id() }))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{SigningKey, VerifyingKey};

    use super::*;

    fn key(seed: u8) -> VerifyingKey {
        SigningKey::from_bytes(&[seed; 32]).verifying_key()
    }

    fn network(peers: &[u8]) -> Network {
        let mut network = Network::default();
        for &seed in peers {
            network.connect(key(seed).into());
        }

        network
    }

    fn send(seed: u8) -> Action {
        Action::Network(NetworkAction::Send { peer: key(seed) })
    }

    #[test]
    fn sends_nothing_without_updates() {
        let actions = Outbox::default().into_actions(&network(&[1, 2]));

        assert_eq!(actions, []);
    }

    #[test]
    fn sends_nothing_without_peers() {
        let mut outbox = Outbox::default();
        outbox.share(Update);

        assert_eq!(outbox.into_actions(&network(&[])), []);
    }

    #[test]
    fn sends_one_message_per_peer() {
        let mut outbox = Outbox::default();
        outbox.share(Update);
        outbox.share(Update);
        outbox.share(Update);

        let actions = outbox.into_actions(&network(&[1, 2]));

        assert_eq!(actions.len(), 2);
        assert!(actions.contains(&send(1)));
        assert!(actions.contains(&send(2)));
    }
}
