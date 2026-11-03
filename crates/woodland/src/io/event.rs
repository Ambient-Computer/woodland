/// Something that happened, locally or remotely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Network(NetworkEvent),
}

/// Something that happened to a peer's connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NetworkEvent {
    Connected,
    Disconnected,
    Message,
}
