use ed25519_dalek::VerifyingKey;

use crate::agreement::AgreementId;
use crate::model::ModelId;
use crate::storage::StorageAction;

/// Something for the caller to do. **Perform them in order.**
///
/// Order is part of the contract. A batch's actions always come out as
/// storage writes first, then reads, then what changed, then sends. So
/// nothing is ever advertised to a peer before it's durable here, every
/// read sees the batch's writes, and so does whoever reads because
/// something changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Storage(StorageAction),
    /// Something a [`Query`] might return is different now.
    ///
    /// [`Query`]: crate::Query
    Changed(Changed),
    Network(NetworkAction),
}

/// What the batch changed in the models, for anyone watching.
///
/// This is how to subscribe to a [`Query`]: keep it, and run it again
/// when a change [affects] it. A change says where to look, not what's
/// there now, and it may be reported when nothing a reader sees is
/// different.
///
/// There are two kinds of read, and a change is to one of them:
///
/// - **Optimistic** reads change when a write arrives, or stops counting.
/// - **Committed** reads change when a write settles. An optimistic read
///   of the same key already showed it, and doesn't change again.
///
/// [`Query`]: crate::Query
/// [affects]: crate::Query::is_affected_by
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Changed {
    /// One key of one model.
    Key {
        agreement: AgreementId,
        model: ModelId,
        key: Vec<u8>,
        /// Whether it's committed reads that changed, or optimistic ones.
        committed: bool,
    },
    /// Anything in the agreement, for either kind of read. The device
    /// caught up from a snapshot, or dropped writes that were discarded.
    Everything { agreement: AgreementId },
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
