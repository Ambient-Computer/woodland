use std::collections::HashSet;

use crate::actor::Peer;

/// This device's view of the network.
#[derive(Debug, Default)]
pub(crate) struct Network {
    /// Peers with an open connection to this device.
    #[expect(dead_code, reason = "used once `Node` processes events")]
    connected: HashSet<Peer>,
}
