//! Signed claims about which view a replica holds, so that witnesses can be
//! relayed instead of heard directly.
//!
//! # Why a relayed claim is as good as a direct one
//!
//! The safety argument (see the module docs) only needs one fact about each
//! witness: at some point it held exactly the learner's view. Views are
//! durable and only grow, so once that's true it stays true. A claim doesn't
//! go stale in any way that matters, and it doesn't matter who delivered it,
//! as long as nobody could have made it up. The signature takes care of
//! that.
//!
//! Before attestations, a witness was the digest of the last view heard
//! directly from a peer. That was already a possibly old claim, vouched for
//! by the transport instead of a signature. Attestations keep the meaning
//! and drop the "directly".

use serde::{Deserialize, Serialize};

use super::{AgreementId, Digest};
use crate::actor::Signable;

/// "I hold the view with this digest", signed by the replica holding it.
///
/// Replicas attest whenever their view changes and relay every attestation
/// they hear, keeping only the latest from each replica.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Attestation {
    /// Which agreement the view belongs to, so a claim can't be replayed
    /// into another one.
    pub(crate) agreement: AgreementId,
    /// Counts this replica's attestations, so relays can tell which is the
    /// latest. Digests can't be ordered, but views only grow, so a later
    /// attestation is never about a smaller view. An older one is still true
    /// (see above). It just stops being useful once everyone has moved on.
    pub(crate) revision: u64,
    pub(crate) digest: Digest,
}

impl Signable for Attestation {
    const CONTEXT: &'static [u8] = b"woodland/agreement/attestation";
}
