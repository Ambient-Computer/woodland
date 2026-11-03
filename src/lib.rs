mod actor;
#[expect(
    dead_code,
    unused_imports,
    reason = "used once `Node` processes events"
)]
mod io;

use io::Network;

/// Represents this device. Applications should only create one.
///
/// Manages current state, updates, and subscriptions.
#[derive(Debug, Default)]
pub struct Node {
    #[expect(dead_code, reason = "used once `Node` processes events")]
    network: Network,
}

impl Node {
    #[must_use]
    pub fn new() -> Self {
        Self {
            network: Network::default(),
        }
    }
}
