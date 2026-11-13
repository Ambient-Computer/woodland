//! What contributors say, each statement signed by whoever said it.
//!
//! A [`View`] is put together from these, so that anyone can pass one on
//! and nobody has to trust who it came from. A device with no write
//! access can relay a view between contributors that never meet, and
//! can't put anything into it: every part is checked against its
//! author's signature, and against whether that author is someone the
//! agreement ever let speak.
//!
//! [`View`]: super::View

use serde::{Deserialize, Serialize};

use super::{AgreementId, Configuration, Decision};
use crate::actor::{Signable, Signed};
use crate::clock::VersionVector;
use crate::lattice::Lattice;

/// "This is what I've integrated, and this is the configuration I had
/// learned when I said so." Each contributor's reports only grow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Report {
    /// So a report can't be replayed into another agreement.
    pub(crate) agreement: AgreementId,
    pub(crate) integrated: VersionVector,
    pub(crate) acknowledged: Configuration,
}

/// "I propose this configuration."
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Proposal {
    pub(crate) agreement: AgreementId,
    pub(crate) configuration: Configuration,
}

/// Everything a replica's view is made of, as it's sent and stored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Gossip {
    pub(crate) learned: Decision,
    pub(crate) proposals: Vec<Signed<Proposal>>,
    pub(crate) reports: Vec<Signed<Report>>,
}

impl Report {
    /// Whether `other` says at least as much.
    pub(crate) fn leq(&self, other: &Self) -> bool {
        self.integrated.leq(&other.integrated) && self.acknowledged.leq(&other.acknowledged)
    }
}

impl Signable for Report {
    const CONTEXT: &'static [u8] = b"woodland/agreement/report";
}

impl Signable for Proposal {
    const CONTEXT: &'static [u8] = b"woodland/agreement/proposal";
}
