use crate::actor::Peer;

/// Something that happened, locally or remotely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Network(NetworkEvent),
}

/// Something that happened to a peer's connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NetworkEvent {
    Connected { peer: Peer },
    Disconnected { peer: Peer },
    Message { peer: Peer },
}
