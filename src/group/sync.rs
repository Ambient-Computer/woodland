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
//! 2. Knowing a peer's clock, send the **transactions** it lacks, in
//!    timeline order, a page at a time. They're read from storage, apart
//!    from the ones integrated in this very batch (see `bodies`). If the
//!    peer lacks some that have settled here, they're gone, so send a
//!    **snapshot** (our settled state) first, and nothing else until it
//!    holds everything we've settled.
//! 3. Send what our agreement **view** is made of whenever it changes,
//!    and every **attestation** newer than what we last sent, whoever
//!    signed it. That's all the agreement protocol needs: views merge,
//!    and matching attestations are witnesses. Every part is signed by
//!    whoever said it, and what was learned comes with proof, so it
//!    doesn't matter who passes it on.
//!
//! A device that [rewinds] says so, and its peers forget what they thought
//! it had.
//!
//! Sends are best-effort. Everything here resets when the peer disconnects,
//! so reconnecting re-sends whatever might have been lost.
//!
//! [rewinds]: super::removal

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::bodies::{Bodies, PAGE};
use super::snapshot::Snapshot;
use crate::actor::{Peer, Signed, Verified};
use crate::agreement::{Agreement, Attestation, Gossip, Proposal, Report, View};
use crate::clock::VersionVector;
use crate::lattice::Lattice;
use crate::log::Log;
use crate::log::Transaction;
use crate::node::Outbox;

/// One agreement's part of a message to a peer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GroupMessage {
    /// Everything the sender's view is made of.
    pub(crate) gossip: Option<Gossip>,
    pub(crate) attestations: Vec<Signed<Attestation>>,
    /// The sender dropped what it had integrated. Its clock, in this
    /// message, replaces whatever it said before.
    pub(crate) rewound: bool,
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
    pub(crate) proposals: Vec<Verified<Proposal>>,
    pub(crate) reports: Vec<Verified<Report>>,
    pub(crate) transactions: Vec<Verified<Transaction>>,
}

/// What we know about one subscribed peer.
#[derive(Debug, Default)]
pub(crate) struct PeerSync {
    /// What the peer has integrated, as far as we know: its last reported
    /// clock, plus everything we've sent since. `None` until it reports.
    has: Option<VersionVector>,
    /// The last view and clock we sent, so we only send changes.
    sent_view: Option<View>,
    /// How many changes of configuration the peer has proof of from us.
    sent_steps: usize,
    sent_clock: Option<VersionVector>,
    /// Whether we rewound and haven't told the peer yet.
    rewound: bool,
    /// The latest revision sent from each attesting replica.
    sent_attestations: BTreeMap<Peer, u64>,
    /// The page of transactions storage was asked for, to send next.
    requested: Vec<(Peer, u64)>,
    /// Whether storage turned out not to hold a transaction it should.
    /// Nothing more is sent until the peer reconnects.
    broken: bool,
}

impl PeerSync {
    /// The peer told us its clock.
    pub(crate) fn heard_clock(&mut self, clock: &VersionVector) {
        self.has.get_or_insert_default().join(clock);
    }

    /// The peer rewound: it has nothing until it says otherwise.
    pub(crate) fn heard_rewind(&mut self) {
        self.has = None;
    }

    /// We rewound. The peer must hear about it.
    pub(crate) fn rewound(&mut self) {
        self.rewound = true;
    }

    /// Whether the peer lacks transactions that have settled here, so only
    /// a snapshot can catch it up.
    pub(crate) fn is_behind(&self, settled: &VersionVector) -> bool {
        self.has.as_ref().is_some_and(|has| !settled.leq(has))
    }

    /// Builds what `peer` needs, and assumes it will arrive. `snapshot` is
    /// our latest, if one is prepared.
    pub(crate) fn outgoing(
        &mut self,
        peer: Peer,
        agreement: &Agreement,
        log: &Log,
        snapshot: Option<&Snapshot>,
        bodies: &mut Bodies,
        outbox: &mut Outbox,
    ) -> Option<GroupMessage> {
        let mut message = GroupMessage::default();

        let view = agreement.view();
        if self.sent_view.as_ref() != Some(view) {
            self.sent_view = Some(view.clone());
            message.gossip = Some(agreement.gossip(self.sent_steps));
            self.sent_steps = agreement.steps();
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

        message.rewound = std::mem::take(&mut self.rewound);
        if message.rewound || self.sent_clock.as_ref() != Some(log.integrated()) {
            self.sent_clock = Some(log.integrated().clone());
            message.clock = Some(log.integrated().clone());
        }

        if let Some(has) = &mut self.has {
            // Our snapshot may trail our cut while a newer one is being
            // prepared. It's still worth sending if it gets the peer closer.
            if let Some(snapshot) = snapshot
                && !log.settled().leq(has)
                && !snapshot.cut.settled.leq(has)
            {
                message.snapshot = Some(snapshot.clone());
                has.join(&snapshot.cut.settled);
            }

            // Not while the peer is still behind: counting what we send as
            // had would hide the gap below our cut, and no snapshot would
            // ever be sent to fill it.
            if log.settled().leq(has) && !self.broken {
                message.transactions = Self::page(log, has, bodies);
            }
        }

        self.request(log, bodies, outbox);

        (message != GroupMessage::default()).then_some(message)
    }

    /// The transactions in hand that the peer can take next, counted as
    /// had from here on. One that isn't in hand holds back the rest of
    /// its author's: a peer's clock can't have gaps.
    fn page(log: &Log, has: &mut VersionVector, bodies: &Bodies) -> Vec<Signed<Transaction>> {
        let mut page = Vec::new();
        for (author, seq) in log.missing_from(has, PAGE) {
            if let Some(transaction) = bodies.get((author, seq))
                && has.next(author) == seq
            {
                has.advance(author, seq);
                page.push(transaction.signed().clone());
            }
        }

        page
    }

    /// Asks storage for the next page, unless the last one is still on
    /// its way.
    fn request(&mut self, log: &Log, bodies: &mut Bodies, outbox: &mut Outbox) {
        let Some(has) = &self.has else {
            return;
        };

        let waiting = !self.requested.iter().all(|&key| bodies.answered(key));
        if waiting || self.broken || !log.settled().leq(has) {
            return;
        }

        self.requested = log.missing_from(has, PAGE);
        self.requested.retain(|&key| bodies.get(key).is_none());
        for &key in &self.requested {
            if bodies.answered(key) {
                tracing::error!("a transaction in the frontier is missing from storage");
                self.broken = true;
            } else {
                bodies.ask(key, outbox);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agreement::Genesis;
    use crate::io::Action;
    use crate::log::Clock;
    use crate::storage::{StorageAction, keys};
    use crate::testing::peers::{identity_of, laptop, phone};

    fn genesis() -> Genesis {
        Genesis::new([7; 32], [phone().id(), laptop().id()])
    }

    /// The keys storage was asked for.
    fn asked(outbox: Outbox) -> Vec<Vec<u8>> {
        let mut actions = outbox.into_actions(Vec::new()).into_iter();
        let get = actions.find_map(|action| match action {
            Action::Storage(StorageAction::Get { keys }) => Some(keys),
            _ => None,
        });

        get.unwrap_or_default()
    }

    /// The phone has more than two pages the laptop lacks, none of them
    /// in hand. It reads a page, sends it, and reads the next.
    #[test]
    fn a_peer_gets_what_its_missing_a_page_at_a_time() {
        let agreement = Agreement::new(&genesis(), phone());
        let mut log = Log::restore(Clock::default(), Vec::new(), phone());
        let mut written = Vec::new();
        for _ in 0..=PAGE * 2 {
            let transaction = log.author(genesis().id(), Vec::new());
            let signed = Signed::sign(&identity_of(phone()), transaction);
            log.offer(signed.clone());
            written.push(signed);
        }

        log.integrate_ready(|_, _| true);
        let stored = |seq: u64| keys::log(genesis().id(), phone(), seq);
        let pages: Vec<Vec<Vec<u8>>> = vec![
            (1..=PAGE as u64).map(stored).collect(),
            (PAGE as u64 + 1..=PAGE as u64 * 2).map(stored).collect(),
        ];

        let mut bodies = Bodies::new(genesis().id());
        let mut sync = PeerSync::default();
        sync.heard_clock(&VersionVector::default());
        let mut outbox = Outbox::default();
        let message = sync.outgoing(laptop(), &agreement, &log, None, &mut bodies, &mut outbox);
        assert_eq!(message.map(|it| it.transactions), Some(Vec::new()));
        assert_eq!(asked(outbox), pages[0]);

        // Nothing more is asked for while that's on its way.
        bodies.clear();
        let mut outbox = Outbox::default();
        sync.outgoing(laptop(), &agreement, &log, None, &mut bodies, &mut outbox);
        assert_eq!(asked(outbox), Vec::<Vec<u8>>::new());

        for transaction in &written[..PAGE] {
            let key = (phone(), transaction.payload().seq);
            bodies.read(key, Some(transaction.clone()));
        }

        let mut outbox = Outbox::default();
        let message = sync.outgoing(laptop(), &agreement, &log, None, &mut bodies, &mut outbox);
        let sent = message.map(|it| it.transactions).unwrap_or_default();
        let page: Vec<_> = written[..PAGE]
            .iter()
            .map(|it| it.signed().clone())
            .collect();
        assert_eq!(sent, page);
        assert_eq!(asked(outbox), pages[1]);
    }
}
