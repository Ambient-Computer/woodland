use crate::actor::Peer;
use crate::agreement::{AgreementId, Change, Genesis};
use crate::model::{ModelId, Operation};

/// Something that happened, locally or remotely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Network(NetworkEvent),
    Storage(StorageEvent),
    Local(LocalEvent),
}

/// Something that happened to a peer's connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NetworkEvent {
    Connected { peer: Peer },
    Disconnected { peer: Peer },
    Message { peer: Peer, bytes: Vec<u8> },
}

/// Something storage did for us.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StorageEvent {
    /// The application asked to load what's stored. Starts hydration.
    Hydrate,
    /// A scan finished.
    Scanned {
        prefix: Vec<u8>,
        entries: Vec<(Vec<u8>, Vec<u8>)>,
    },
}

/// Something the application did on this device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LocalEvent {
    /// Start replicating an agreement, as a founder or an invitee.
    Join { genesis: Genesis },
    /// Propose a membership change.
    Reconfigure {
        agreement: AgreementId,
        change: Change,
    },
    /// Commit a transaction.
    Write {
        agreement: AgreementId,
        model: ModelId,
        operations: Vec<Operation>,
    },
}
