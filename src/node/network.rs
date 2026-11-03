use std::collections::HashSet;

use crate::actor::Peer;

/// This device's view of the network.
#[derive(Debug, Default)]
pub(crate) struct Network {
    /// Peers with an open connection to this device.
    connected: HashSet<Peer>,
}

impl Network {
    pub(crate) fn connect(&mut self, peer: Peer) {
        self.connected.insert(peer);
    }

    pub(crate) fn disconnect(&mut self, peer: &Peer) {
        self.connected.remove(peer);
    }

    pub(crate) fn connected(&self) -> impl Iterator<Item = &Peer> {
        self.connected.iter()
    }

    #[cfg(test)]
    pub(crate) fn is_connected(&self, peer: &Peer) -> bool {
        self.connected.contains(peer)
    }
}
