use ed25519_dalek::VerifyingKey;

/// Something for the caller to do. Perform them in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Network(NetworkAction),
}

/// Something for the caller to do over the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkAction {
    /// Best-effort. Drop it if the peer is gone or backed up.
    Send { peer: VerifyingKey },
}
