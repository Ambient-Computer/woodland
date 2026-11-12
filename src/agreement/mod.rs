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
//! 1. **Propose** by joining into your own view.
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
//! - **Crash faults only.** Nobody can forge another replica's attestation,
//!   but a lying member could attest to a view it doesn't hold, or forge
//!   another's progress inside a view. Byzantine-tolerant RLA is follow-up
//!   work in the literature.

mod attestation;
mod configuration;
mod genesis;
mod view;

use std::collections::{BTreeMap, BTreeSet};

pub(crate) use attestation::Attestation;
pub(crate) use configuration::{Change, Configuration};
pub use genesis::{AgreementId, Genesis};
pub(crate) use view::{Decision, Digest, View};

use crate::actor::{Identity, Peer, Signed, Verified};
use crate::clock::Progress;
use crate::lattice::Lattice;

/// One replica of one agreement, as seen from this device.
#[derive(Debug)]
pub(crate) struct Agreement {
    id: AgreementId,
    /// This device. It witnesses its own view when it's a member.
    me: Peer,
    view: View,
    /// The latest attestation from each replica, this one included, however
    /// it arrived. A replica witnesses our view when its digest matches ours.
    heard: BTreeMap<Peer, Verified<Attestation>>,
}

impl Agreement {
    pub(crate) fn new(genesis: &Genesis, me: Peer) -> Self {
        Self::resume(genesis.id(), View::new(genesis.configuration()), None, me)
    }

    /// Picks up from a view and attestation saved earlier. Nothing else
    /// heard is remembered, so peers must attest again before anything new
    /// is learned.
    pub(crate) fn resume(
        id: AgreementId,
        view: View,
        attested: Option<Verified<Attestation>>,
        me: Peer,
    ) -> Self {
        let heard = attested.map(|it| (me, it)).into_iter().collect();

        Self {
            id,
            me,
            view,
            heard,
        }
    }

    pub(crate) fn view(&self) -> &View {
        &self.view
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

    /// Proposes progress, typically this device's own report.
    pub(crate) fn propose_progress(&mut self, progress: &Progress) -> bool {
        self.view.candidate.join(progress)
    }

    /// Proposes that this replica has learned the configuration it has.
    /// Goes with a progress report: it's what lets the report count toward
    /// the floor of that configuration (see [`Decision::floor`]).
    pub(crate) fn acknowledge(&mut self) -> bool {
        let learned = self.learned().configuration.clone();

        self.view
            .acknowledged
            .join(&BTreeMap::from([(self.me, learned)]))
    }

    /// Proposes a membership change on top of what's been learned.
    pub(crate) fn propose_change(&mut self, change: Change) -> bool {
        let before = self.view.clone();
        self.view.propose(self.learned().configuration.with(change));

        self.view != before
    }

    /// Merges a peer's view. What it holds is the business of its
    /// attestation, which may arrive separately or through someone else.
    pub(crate) fn receive(&mut self, view: &View) -> bool {
        self.view.merge(view)
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
        Some(&self.view.learned)
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
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::actor::verify_all;
    use crate::clock::VersionVector;
    use crate::testing::peers::{friend, identity_of, laptop, nas, phone};

    /// The household founds the agreement. The friend holds the genesis but
    /// isn't a member until someone adds them.
    fn genesis() -> Genesis {
        Genesis::new([7; 32], [phone().id(), laptop().id(), nas().id()])
    }

    fn cast() -> [Peer; 4] {
        [phone(), laptop(), nas(), friend()]
    }

    /// What one replica sends another: its view, and every attestation it
    /// holds, its own refreshed first. This is what the group sends too.
    #[derive(Clone, Debug)]
    struct Gossip {
        view: View,
        attestations: Vec<Signed<Attestation>>,
    }

    fn gossip(from: &mut Agreement) -> Gossip {
        from.attest(&identity_of(from.me));
        let attestations = from.attestations().map(|it| it.signed().clone());

        Gossip {
            view: from.view().clone(),
            attestations: attestations.collect(),
        }
    }

    fn deliver(to: &mut Agreement, gossip: Gossip) {
        to.receive(&gossip.view);
        for attestation in verify_all(gossip.attestations) {
            to.hear(attestation);
        }
    }

    fn send(from: &mut Agreement, to: &mut Agreement) {
        deliver(to, gossip(from));
    }

    #[derive(Clone, Copy, Debug)]
    enum Step {
        /// A replica reports more of its own progress.
        Report { replica: usize, by: u64 },
        /// A replica proposes a membership change.
        Reconfigure { replica: usize, change: usize },
        /// A replica sends its current view and the attestations it holds.
        /// They may arrive much later.
        Send { from: usize, to: usize },
        /// An in-flight message arrives, and maybe stays in flight to arrive
        /// again (duplication).
        Deliver { message: usize, duplicate: bool },
        /// An in-flight message is lost.
        Drop { message: usize },
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            (0..4usize, 1..3u64).prop_map(|(replica, by)| Step::Report { replica, by }),
            (0..4usize, 0..3usize)
                .prop_map(|(replica, change)| Step::Reconfigure { replica, change }),
            (0..4usize, 1..4usize).prop_map(|(from, offset)| Step::Send {
                from,
                to: (from + offset) % 4
            }),
            (any::<usize>(), any::<bool>())
                .prop_map(|(message, duplicate)| Step::Deliver { message, duplicate }),
            any::<usize>().prop_map(|message| Step::Drop { message }),
        ]
    }

    /// The changes anyone may propose. Never enough removals to empty the
    /// configuration: an empty configuration can't learn, by design.
    fn change(index: usize) -> Change {
        [
            Change::Add(friend()),
            Change::Remove(nas()),
            Change::Remove(phone()),
        ][index]
    }

    /// Four replicas, an unreliable network, and a record of everything
    /// proposed and learned.
    ///
    /// Every message relays attestations, so replicas often count witnesses
    /// they never heard from directly.
    struct Simulation {
        replicas: Vec<Agreement>,
        /// Each message, with the replica it's addressed to.
        in_flight: Vec<(usize, Gossip)>,
        reported: Vec<u64>,
        proposed: Decision,
        learned: Vec<Decision>,
    }

    impl Simulation {
        fn new() -> Self {
            let replicas = cast().map(|peer| Agreement::new(&genesis(), peer)).into();

            Self {
                replicas,
                in_flight: Vec::new(),
                reported: vec![0; 4],
                proposed: View::new(genesis().configuration()).learned,
                learned: Vec::new(),
            }
        }

        fn run(&mut self, step: Step) {
            match step {
                Step::Report { replica, by } => {
                    self.reported[replica] += by;
                    let me = cast()[replica];
                    let clock = VersionVector::from_iter([(me, self.reported[replica])]);
                    let report = Progress::report(me, clock);

                    self.proposed.progress.join(&report);
                    self.replicas[replica].propose_progress(&report);
                }

                Step::Reconfigure {
                    replica,
                    change: index,
                } => {
                    let agreement = &mut self.replicas[replica];
                    agreement.propose_change(change(index));

                    let proposal = agreement.learned().configuration.with(change(index));
                    self.proposed.configuration.join(&proposal);
                }

                Step::Send { from, to } => {
                    let message = gossip(&mut self.replicas[from]);
                    self.in_flight.push((to, message));
                }

                Step::Deliver { message, duplicate } => {
                    if self.in_flight.is_empty() {
                        return;
                    }

                    let index = message % self.in_flight.len();
                    let (to, message) = if duplicate {
                        self.in_flight[index].clone()
                    } else {
                        self.in_flight.swap_remove(index)
                    };

                    deliver(&mut self.replicas[to], message);
                    self.learn(to);
                }

                Step::Drop { message } => {
                    if !self.in_flight.is_empty() {
                        let index = message % self.in_flight.len();
                        self.in_flight.swap_remove(index);
                    }
                }
            }
        }

        fn learn(&mut self, replica: usize) {
            if let Some(decision) = self.replicas[replica].try_learn() {
                self.learned.push(decision.clone());
            }
        }

        /// Everyone reconnects and exchanges views until nothing changes.
        fn heal(&mut self) {
            for _ in 0..20 {
                let messages: Vec<Gossip> = self.replicas.iter_mut().map(gossip).collect();
                for (to, replica) in self.replicas.iter_mut().enumerate() {
                    for (from, message) in messages.iter().enumerate() {
                        if from != to {
                            deliver(replica, message.clone());
                        }
                    }
                }

                let views: Vec<&View> = messages.iter().map(|it| &it.view).collect();

                for replica in 0..self.replicas.len() {
                    self.learn(replica);
                }

                let settled = self
                    .replicas
                    .iter()
                    .all(|it| it.view() == views[0] && it.learned() == &self.proposed);
                if settled {
                    return;
                }
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// Safety: whatever the schedule, no two learned values disagree.
        #[test]
        fn learned_values_are_always_comparable(steps in prop::collection::vec(step(), 0..120)) {
            let mut simulation = Simulation::new();
            for step in steps {
                simulation.run(step);
            }

            for left in &simulation.learned {
                for right in &simulation.learned {
                    prop_assert!(
                        left.leq(right) || right.leq(left),
                        "incomparable decisions:\n{left:?}\n{right:?}",
                    );
                }
            }
        }

        /// Validity: nothing is learned that nobody proposed.
        #[test]
        fn learned_values_were_proposed(steps in prop::collection::vec(step(), 0..120)) {
            let mut simulation = Simulation::new();
            for step in steps {
                simulation.run(step);
            }

            for decision in &simulation.learned {
                prop_assert!(decision.leq(&simulation.proposed));
            }
        }

        /// Liveness: once proposals stop and everyone can talk, every replica
        /// (the friend included, member or not) learns everything proposed.
        #[test]
        fn quiet_connected_replicas_learn_everything(steps in prop::collection::vec(step(), 0..120)) {
            let mut simulation = Simulation::new();
            for step in steps {
                simulation.run(step);
            }

            simulation.heal();

            for replica in &simulation.replicas {
                prop_assert_eq!(replica.learned(), &simulation.proposed);
            }
        }
    }

    #[test]
    fn a_lone_member_cannot_learn() {
        let mut phone_replica = Agreement::new(&genesis(), phone());
        let report = Progress::report(phone(), VersionVector::from_iter([(phone(), 1)]));
        phone_replica.propose_progress(&report);

        assert_eq!(phone_replica.try_learn(), None);
    }

    #[test]
    fn two_of_three_members_can_learn_while_one_is_offline() {
        let mut phone_replica = Agreement::new(&genesis(), phone());
        let mut laptop_replica = Agreement::new(&genesis(), laptop());
        let report = Progress::report(phone(), VersionVector::from_iter([(phone(), 1)]));
        phone_replica.propose_progress(&report);

        send(&mut phone_replica, &mut laptop_replica);
        send(&mut laptop_replica, &mut phone_replica);

        let learned = phone_replica.try_learn().cloned();
        assert_eq!(learned.map(|it| it.progress), Some(report));
    }

    /// Found by the proptest when learning only counted the target
    /// configuration. The laptop proposes removing everyone else, which
    /// makes it a majority of one in the configuration it's aiming for. If
    /// that were enough, it would learn alone while the phone and the nas,
    /// still a majority of the household, learn something else.
    ///
    /// A new configuration can't vouch for itself. The configurations it
    /// grows out of have to agree too.
    #[test]
    fn a_reconfiguration_needs_the_old_quorum_too() {
        let mut laptop_replica = Agreement::new(&genesis(), laptop());
        laptop_replica.propose_change(Change::Remove(nas()));
        laptop_replica.propose_change(Change::Remove(phone()));

        let target = laptop_replica.view().target_configuration();
        assert!(target.has_quorum(&BTreeSet::from([laptop()])));
        assert_eq!(laptop_replica.try_learn(), None);
    }

    /// Why "storage writes are durable before the next action" is a safety
    /// requirement, not just a data-loss concern.
    ///
    /// Safety rests on each replica's view only ever growing. Here the
    /// laptop witnesses the phone's proposal, then crashes before its view
    /// is durable and comes back with an older one. It then witnesses the
    /// nas's proposal. Each was a majority with the laptop, so the phone
    /// and the nas learn values that disagree.
    #[test]
    fn a_replica_that_forgets_breaks_agreement() {
        let report = |peer| Progress::report(peer, VersionVector::from_iter([(peer, 1)]));
        let mut phone_replica = Agreement::new(&genesis(), phone());
        let mut laptop_replica = Agreement::new(&genesis(), laptop());
        let mut nas_replica = Agreement::new(&genesis(), nas());

        phone_replica.propose_progress(&report(phone()));
        send(&mut phone_replica, &mut laptop_replica);
        send(&mut laptop_replica, &mut phone_replica);
        let phone_learned = phone_replica.try_learn().cloned();

        let amnesiac = Agreement::new(&genesis(), laptop());
        laptop_replica = amnesiac;

        nas_replica.propose_progress(&report(nas()));
        send(&mut nas_replica, &mut laptop_replica);
        send(&mut laptop_replica, &mut nas_replica);
        let nas_learned = nas_replica.try_learn().cloned();

        let (Some(phone_learned), Some(nas_learned)) = (phone_learned, nas_learned) else {
            panic!("both should learn");
        };
        assert!(!phone_learned.leq(&nas_learned) && !nas_learned.leq(&phone_learned));
    }

    #[test]
    fn adding_a_member_raises_the_quorum() {
        let mut phone_replica = Agreement::new(&genesis(), phone());
        let mut laptop_replica = Agreement::new(&genesis(), laptop());
        phone_replica.propose_change(Change::Add(friend()));
        send(&mut phone_replica, &mut laptop_replica);
        send(&mut laptop_replica, &mut phone_replica);

        // Two of three household members agree, but the configuration with
        // the friend has four members, so it needs three.
        assert_eq!(phone_replica.try_learn(), None);

        let mut friend_replica = Agreement::new(&genesis(), friend());
        send(&mut phone_replica, &mut friend_replica);
        send(&mut friend_replica, &mut phone_replica);

        let learned = phone_replica.try_learn().cloned();
        assert!(learned.is_some_and(|it| it.configuration.is_member(friend())));
    }

    /// The phone and the nas never meet. The friend, who isn't a member and
    /// never tries to learn, carries their views and attestations between
    /// them. The phone learns by counting the nas's attestation, which it
    /// only ever got secondhand.
    #[test]
    fn a_relayed_attestation_counts() {
        let mut phone_replica = Agreement::new(&genesis(), phone());
        let mut nas_replica = Agreement::new(&genesis(), nas());
        let mut friend_replica = Agreement::new(&genesis(), friend());
        let report = Progress::report(phone(), VersionVector::from_iter([(phone(), 1)]));
        phone_replica.propose_progress(&report);

        send(&mut phone_replica, &mut friend_replica);
        send(&mut friend_replica, &mut nas_replica);
        send(&mut nas_replica, &mut friend_replica);
        send(&mut friend_replica, &mut phone_replica);

        let learned = phone_replica.try_learn().cloned();
        assert_eq!(learned.map(|it| it.progress), Some(report));
    }

    /// Two agreements founded by the same devices start from the same view,
    /// so their digests match. The attestation names its agreement, so it
    /// can't be replayed from one into the other.
    #[test]
    fn attestations_only_count_in_their_own_agreement() {
        let elsewhere = Genesis::new([8; 32], [phone().id(), laptop().id(), nas().id()]);
        let mut phone_replica = Agreement::new(&genesis(), phone());
        let mut laptop_elsewhere = Agreement::new(&elsewhere, laptop());
        let report = Progress::report(phone(), VersionVector::from_iter([(phone(), 1)]));
        phone_replica.propose_progress(&report);
        laptop_elsewhere.propose_progress(&report);
        assert_eq!(
            phone_replica.view().digest(),
            laptop_elsewhere.view().digest()
        );

        send(&mut laptop_elsewhere, &mut phone_replica);

        assert_eq!(phone_replica.try_learn(), None);
    }
}
