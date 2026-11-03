use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::agreement::AgreementId;
use crate::group::GroupMessage;

/// The one message a node sends a peer per batch.
///
/// It isn't signed as a whole. The transport (Iroh) already authenticates
/// which peer sent it, so the envelope only needs to be trusted as far as
/// "this peer said this". Transactions inside are signed by their authors,
/// because those get stored and relayed to third parties.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Message {
    /// The agreements this node replicates, sent when connecting and
    /// whenever the set changes. Overlap with the peer's own set decides
    /// what the two of them sync.
    pub(crate) hello: Option<BTreeSet<AgreementId>>,
    pub(crate) groups: BTreeMap<AgreementId, GroupMessage>,
}
