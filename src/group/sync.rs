//! What one agreement sends to one peer.
//!
//! Domains don't pick recipients, but somebody has to decide what each peer
//! is missing. That happens here, once per batch per peer, so the node can
//! bundle everything into a single message.
//!
//! The exchange, per agreement:
//!
//! 1. Both sides send their **clock** when it changes. A peer's clock says
//!    exactly which transactions it has.
//! 2. Knowing a peer's clock, send the **transactions** it lacks. If it's
//!    behind our floor, the transactions it needs are gone, so send a
//!    **snapshot** (our model state) first.
//! 3. Send our agreement **view** whenever it changes, and every
//!    **attestation** newer than what we last sent, whoever signed it.
//!    That's all the agreement protocol needs: views merge, and matching
//!    attestations are witnesses.
//!
//! Sends are best-effort. Everything here resets when the peer disconnects,
//! so reconnecting re-sends whatever might have been lost.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::history::History;
use crate::actor::{Peer, Signed, Verified};
use crate::agreement::{Agreement, Attestation, View};
use crate::clock::VersionVector;
use crate::lattice::Lattice;
use crate::model::{KvModel, ModelId, Transaction};

/// One agreement's part of a message to a peer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GroupMessage {
    pub(crate) view: Option<View>,
    pub(crate) attestations: Vec<Signed<Attestation>>,
    pub(crate) clock: Option<VersionVector>,
    pub(crate) snapshot: Option<Snapshot>,
    pub(crate) transactions: Vec<Signed<Transaction>>,
}

/// The signed parts of a [`GroupMessage`], once their signatures have been
/// checked. The node checks every signature in a message before any group
/// sees it.
#[derive(Debug, Default)]
pub(crate) struct VerifiedParts {
    pub(crate) attestations: Vec<Verified<Attestation>>,
    pub(crate) transactions: Vec<Verified<Transaction>>,
}

/// The state of every model, for a peer that has fallen behind our floor.
///
/// It reflects every transaction we've integrated, not just those below the
/// floor. That's fine: the transactions above the floor travel with it, and
/// applying a last-writer-wins write twice changes nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub(crate) floor: VersionVector,
    pub(crate) lamport: u64,
    pub(crate) models: BTreeMap<ModelId, KvModel>,
}

/// What we know about one subscribed peer.
#[derive(Debug, Default)]
pub(crate) struct PeerSync {
    /// What the peer has integrated, as far as we know: its last reported
    /// clock, plus everything we've sent since. `None` until it reports.
    has: Option<VersionVector>,
    /// The last view and clock we sent, so we only send changes.
    sent_view: Option<View>,
    sent_clock: Option<VersionVector>,
    /// The latest revision sent from each attesting replica.
    sent_attestations: BTreeMap<Peer, u64>,
}

impl PeerSync {
    /// The peer told us its clock.
    pub(crate) fn heard_clock(&mut self, clock: &VersionVector) {
        self.has.get_or_insert_default().join(clock);
    }

    /// Builds what `peer` needs, and assumes it will arrive.
    pub(crate) fn outgoing(
        &mut self,
        peer: Peer,
        agreement: &Agreement,
        history: &History,
        models: &BTreeMap<ModelId, KvModel>,
    ) -> Option<GroupMessage> {
        let mut message = GroupMessage::default();

        let view = agreement.view();
        if self.sent_view.as_ref() != Some(view) {
            self.sent_view = Some(view.clone());
            message.view = Some(view.clone());
        }

        // The peer's own attestations would only come back to it.
        for attestation in agreement.attestations() {
            let (author, revision) = (attestation.author(), attestation.payload().revision);
            let sent = self.sent_attestations.entry(author).or_default();
            if author != peer && *sent < revision {
                *sent = revision;
                message.attestations.push(attestation.signed().clone());
            }
        }

        if self.sent_clock.as_ref() != Some(history.integrated()) {
            self.sent_clock = Some(history.integrated().clone());
            message.clock = Some(history.integrated().clone());
        }

        if let Some(has) = &mut self.has {
            if !history.floor().leq(has) {
                message.snapshot = Some(Snapshot {
                    floor: history.floor().clone(),
                    lamport: history.clock().lamport,
                    models: models.clone(),
                });
                has.join(history.floor());
            }

            message.transactions = history.missing_from(has);
            has.join(history.integrated());
        }

        (message != GroupMessage::default()).then_some(message)
    }
}
