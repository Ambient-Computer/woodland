//! Reconfigurable Lattice Agreement (RLA): how contributors agree on who
//! they are, and on how far each of them has gotten.
//!
//! Based on Kuznetsov, Rieutord and Tucci-Piergiovanni, "Reconfigurable
//! Lattice Agreement and Applications" (OPODIS 2019, arXiv:1910.09264).
//!
//! # What it promises
//!
//! Replicas propose values from a lattice. Each replica learns a growing
//! series of values, and **any two values learned anywhere are comparable**:
//! one includes the other. There's no leader, no ballots and no single log.
//! Unlike consensus, proposals never conflict: they merge.
//!
//! The configuration (who the contributors are) is itself a lattice of
//! membership changes, carried inside every learned value. Reconfiguring is
//! just proposing a bigger configuration.
//!
//! # How it works here
//!
//! Each replica keeps a [`View`]: what it has learned, the join of every
//! value proposed, and the configurations proposed but not yet learned.
//!
//! 1. **Propose** by signing a statement and joining it into your own
//!    view.
//! 2. **Gossip** your view whenever it changes. Whoever receives it merges.
//!    Merging is the only thing a replica ever does with a view.
//! 3. **Attest** whenever your view changes: sign "I hold the view with this
//!    digest". Pass on every [`Attestation`] you hear, so a claim reaches
//!    devices its author never meets.
//! 4. **Witness.** A peer whose latest attestation matches your digest is
//!    holding exactly what you hold.
//! 5. **Learn** your candidate once a quorum of _every configuration in play_
//!    witnesses it. Learning joins the candidate into `learned`, which then
//!    spreads by gossip like everything else.
//!
//! Why learned values are comparable: two learners each had a quorum
//! holding their exact view. Some configuration they both counted has
//! quorums that overlap in one replica. That replica's view only ever
//! grows, so the two views it held, and therefore the two learned values,
//! are ordered.
//!
//! "Every configuration in play" is the learned configuration joined with
//! every subset of the pending proposals. A replica can't know which of the
//! proposals another replica has heard of, so it covers all the
//! combinations.
//!
//! # Where it departs from the paper
//!
//! The paper runs request/response rounds and checks that responses didn't
//! change anything. Here, a replica instead witnesses by holding an equal
//! view, so there are no rounds. Any attestation counts, however old it is
//! and whoever relayed it. That makes the protocol pure state-based gossip,
//! which suits devices that are offline for weeks and peers that never meet.
//!
//! # What it needs
//!
//! - **Safety** (never learning incomparable values) holds no matter who is
//!   offline or how messages are delayed, dropped or reordered.
//! - **Durable views.** Safety assumes a replica's view never shrinks, so a
//!   view must be persisted before it's sent. A replica that forgets what
//!   it sent can witness two incomparable values (see the test
//!   `a_replica_that_forgets_breaks_agreement`). The node's action ordering,
//!   storage before network, is what provides this. A replica's own latest
//!   attestation is persisted with its view for a liveness reason: relays
//!   keep the highest revision they've seen, so a replica that restarted
//!   counting from 1 would be ignored by all of them.
//! - **Liveness** needs a quorum of every configuration in play to be
//!   reachable, and needs proposals to pause long enough for views to
//!   converge. A stream of proposals starves learning in practice, not just
//!   in theory, so the group proposes at most one progress report at a time
//!   (see `Group::may_report`).
//! - **Connectivity, not adjacency.** Views, decisions and attestations all
//!   spread hop by hop, so learning only needs the devices to be connected
//!   over time, in any shape. Attestations are what make that true. When
//!   witnesses had to be heard directly, some device had to be adjacent to a
//!   quorum. That holds for almost any handful of devices (see
//!   `a_hub_that_isnt_a_contributor_learns_for_everyone`), but not for six
//!   contributors in a line.
//! - **Honest contributors.** Nobody can forge what a contributor said:
//!   reports, proposals and attestations are all signed (see
//!   `statement`), so anyone can relay a view. But a lying contributor
//!   could attest to a view it doesn't hold, or report progress it
//!   hasn't made. Byzantine-tolerant RLA is follow-up work in the
//!   literature.

mod attestation;
mod configuration;
mod genesis;
mod statement;
mod view;

use std::collections::{BTreeMap, BTreeSet};

pub(crate) use attestation::Attestation;
pub(crate) use configuration::{Change, Configuration};
pub use genesis::{AgreementId, Genesis};
pub(crate) use statement::{Gossip, Proposal, Report};
pub(crate) use view::{Decision, Digest, View};

use crate::actor::{Identity, Peer, Signed, Verified};
use crate::clock::{Progress, VersionVector};
use crate::lattice::Lattice;

/// One replica of one agreement, as seen from this device.
#[derive(Debug)]
pub(crate) struct Agreement {
    id: AgreementId,
    /// This device. It witnesses its own view when it's a member.
    me: Peer,
    view: View,
    /// The latest report from each contributor. The view's candidate and
    /// acknowledgements are exactly what these say.
    reports: BTreeMap<Peer, Verified<Report>>,
    /// Who proposed each of the view's pending configurations.
    proposals: BTreeMap<Configuration, Verified<Proposal>>,
    /// The latest attestation from each replica, this one included, however
    /// it arrived. A replica witnesses our view when its digest matches ours.
    heard: BTreeMap<Peer, Verified<Attestation>>,
}

impl Agreement {
    pub(crate) fn new(genesis: &Genesis, me: Peer) -> Self {
        let founded = Gossip {
            learned: View::new(genesis.configuration()).learned,
            ..Gossip::default()
        };

        Self::resume(genesis.id(), founded, None, me)
    }

    /// Picks up from what was saved earlier: everything the view was made
    /// of, and this replica's attestation. It was all checked before it
    /// was stored. Nothing else heard is remembered, so peers must attest
    /// again before anything new is learned.
    pub(crate) fn resume(
        id: AgreementId,
        saved: Gossip,
        attested: Option<Verified<Attestation>>,
        me: Peer,
    ) -> Self {
        let mut agreement = Self {
            id,
            me,
            view: View::new(saved.learned.configuration.clone()),
            reports: BTreeMap::new(),
            proposals: BTreeMap::new(),
            heard: attested.map(|it| (me, it)).into_iter().collect(),
        };

        let proposals = saved.proposals.into_iter().map(Verified::trust_stored);
        let reports = saved.reports.into_iter().map(Verified::trust_stored);
        agreement.receive(&saved.learned, proposals.collect(), reports.collect());

        agreement
    }

    pub(crate) fn view(&self) -> &View {
        &self.view
    }

    /// Everything the view is made of, to send or to store.
    pub(crate) fn gossip(&self) -> Gossip {
        let proposals = self.proposals.values();
        let reports = self.reports.values();

        Gossip {
            learned: self.view.learned.clone(),
            proposals: proposals.map(|it| it.signed().clone()).collect(),
            reports: reports.map(|it| it.signed().clone()).collect(),
        }
    }

    /// This replica's latest attestation, if it has made one.
    pub(crate) fn attested(&self) -> Option<&Verified<Attestation>> {
        self.heard.get(&self.me)
    }

    /// Every attestation worth passing on.
    pub(crate) fn attestations(&self) -> impl Iterator<Item = &Verified<Attestation>> {
        self.heard.values()
    }

    /// The greatest value known to be learned.
    pub(crate) fn learned(&self) -> &Decision {
        &self.view.learned
    }

    /// Proposes this device's progress, and that it has learned the
    /// configuration it has. The second part is what lets the first count
    /// toward the floor of that configuration (see [`Decision::floor`]).
    /// `identity` must be this replica's own.
    pub(crate) fn report(&mut self, identity: &Identity, integrated: VersionVector) -> bool {
        let report = Report {
            agreement: self.id,
            integrated,
            acknowledged: self.learned().configuration.clone(),
        };

        self.accept_report(Signed::sign(identity, report))
    }

    /// Proposes a membership change on top of what's been learned.
    /// `identity` must be this replica's own.
    pub(crate) fn propose_change(&mut self, identity: &Identity, change: Change) -> bool {
        let proposal = Proposal {
            agreement: self.id,
            configuration: self.learned().configuration.with(change),
        };

        self.accept_proposal(Signed::sign(identity, proposal))
    }

    /// Merges what a peer holds. Every part arrives with its author's
    /// signature checked, and is kept only if its author may speak. What
    /// the peer itself holds is the business of its attestation, which
    /// may arrive separately or through someone else.
    pub(crate) fn receive(
        &mut self,
        learned: &Decision,
        mut proposals: Vec<Verified<Proposal>>,
        reports: Vec<Verified<Report>>,
    ) -> bool {
        let before = self.view.clone();
        self.view.learn(learned);

        // One proposal can be what lets the author of another speak, so
        // go around until no more of them are accepted.
        loop {
            let waiting = proposals.len();
            proposals.retain(|proposal| !self.accept_proposal(proposal.clone()));
            if proposals.len() == waiting {
                break;
            }
        }

        for report in reports {
            self.accept_report(report);
        }

        self.forget_superseded();
        self.view != before
    }

    /// Keeps an attestation if it's the latest from its author. Ones for
    /// other agreements are ignored.
    pub(crate) fn hear(&mut self, attestation: Verified<Attestation>) -> bool {
        let claim = attestation.payload();
        let newer = self
            .heard
            .get(&attestation.author())
            .is_none_or(|held| held.payload().revision < claim.revision);

        if claim.agreement != self.id || !newer {
            return false;
        }

        self.heard.insert(attestation.author(), attestation);
        true
    }

    /// Signs what this replica holds, if that changed since it last said.
    /// `identity` must be this replica's own.
    ///
    /// Call it after the view settles and before it's sent. The attestation
    /// must be persisted along with the view (see the module docs).
    pub(crate) fn attest(&mut self, identity: &Identity) -> bool {
        let digest = self.view.digest();
        let previous = self.attested().map(Verified::payload);
        if previous.is_some_and(|it| it.digest == digest) {
            return false;
        }

        let attestation = Attestation {
            agreement: self.id,
            revision: previous.map_or(1, |it| it.revision + 1),
            digest,
        };

        self.heard
            .insert(self.me, Signed::sign(identity, attestation));
        true
    }

    /// Learns the candidate if a quorum of every configuration in play
    /// holds this exact view. Returns the new decision, if there is one.
    pub(crate) fn try_learn(&mut self) -> Option<&Decision> {
        let holders = self.witnesses();
        let every_quorum_holds = self
            .view
            .configurations_in_play()
            .iter()
            .all(|configuration| configuration.has_quorum(&holders));

        let decision = Decision {
            progress: self.view.candidate.clone(),
            acknowledged: self.view.acknowledged.clone(),
            configuration: self.view.target_configuration(),
        };

        if !every_quorum_holds || decision.leq(&self.view.learned) {
            return None;
        }

        self.view.learn(&decision);
        self.forget_superseded();
        Some(&self.view.learned)
    }

    /// Whether `peer` may report and propose: it was a contributor at
    /// some point, in a configuration that's learned or proposed.
    ///
    /// That only ever becomes true, which matters. If replicas could
    /// disagree for good about whether a statement counts, their views
    /// would never match again, and nothing more would be learned. So a
    /// removed contributor can still speak. Whether its word is used is
    /// decided where it's read (see `Decision::floor`).
    fn may_speak(&self, peer: Peer) -> bool {
        self.view.target_configuration().was_ever_member(peer)
    }

    /// Returns whether the proposal counts, even if what's learned
    /// already covers it.
    fn accept_proposal(&mut self, proposal: Verified<Proposal>) -> bool {
        let claim = proposal.payload();
        if claim.agreement != self.id || !self.may_speak(proposal.author()) {
            return false;
        }

        let configuration = claim.configuration.clone();
        self.view.propose(configuration.clone());
        if self.view.proposals.contains(&configuration) {
            self.proposals.entry(configuration).or_insert(proposal);
        }

        true
    }

    /// Returns whether the report said anything new.
    fn accept_report(&mut self, report: Verified<Report>) -> bool {
        let (author, claim) = (report.author(), report.payload());
        let held = self.reports.get(&author).map(Verified::payload);
        let newer = held.is_none_or(|held| held != claim && held.leq(claim));
        if claim.agreement != self.id || !self.may_speak(author) || !newer {
            return false;
        }

        let progress = Progress::report(author, claim.integrated.clone());
        let acknowledged = BTreeMap::from([(author, claim.acknowledged.clone())]);
        self.view.candidate.join(&progress);
        self.view.acknowledged.join(&acknowledged);
        self.reports.insert(author, report);
        true
    }

    /// Drops the signatures of proposals the view no longer holds.
    fn forget_superseded(&mut self) {
        let pending = &self.view.proposals;
        self.proposals
            .retain(|configuration, _| pending.contains(configuration));
    }

    /// Replicas known to have held exactly our view, ourselves included.
    /// Our own attestation may lag the view, but we hold it regardless.
    fn witnesses(&self) -> BTreeSet<Peer> {
        let ours = self.view.digest();
        let peers = self
            .heard
            .iter()
            .filter(|&(_, attestation)| attestation.payload().digest == ours)
            .map(|(&peer, _)| peer);

        peers.chain([self.me]).collect()
    }
}

#[cfg(test)]
mod tests;
