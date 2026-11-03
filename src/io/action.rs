use ed25519_dalek::VerifyingKey;

use crate::storage::StorageAction;

/// Something for the caller to do. **Perform them in order.**
///
/// Order is part of the contract. A batch's actions always come out as
/// storage writes first, then scans, then sends, so nothing is ever
/// advertised to a peer before it's durable here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Storage(StorageAction),
    Network(NetworkAction),
}

/// Something for the caller to do over the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkAction {
    /// Best-effort. Drop it if the peer is gone or backed up. Nothing is
    /// lost for good: peers re-sync whenever they reconnect.
    Send {
        peer: VerifyingKey,
        message: Vec<u8>,
    },
}
