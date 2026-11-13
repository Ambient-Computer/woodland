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
struct Sent {
    view: View,
    gossip: Gossip,
    attestations: Vec<Signed<Attestation>>,
}

fn gossip(from: &mut Agreement) -> Sent {
    from.attest(&identity_of(from.me));
    let attestations = from.attestations().map(|it| it.signed().clone());

    Sent {
        view: from.view().clone(),
        gossip: from.gossip(),
        attestations: attestations.collect(),
    }
}

fn deliver(to: &mut Agreement, sent: Sent) {
    let Gossip {
        learned,
        proposals,
        reports,
    } = sent.gossip;
    to.receive(&learned, verify_all(proposals), verify_all(reports));
    for attestation in verify_all(sent.attestations) {
        to.hear(attestation);
    }
}

/// A replica reports having integrated `count` of its own transactions.
fn report(replica: &mut Agreement, count: u64) -> Progress {
    let me = replica.me;
    let clock = VersionVector::from_iter([(me, count)]);
    replica.report(&identity_of(me), clock.clone());

    Progress::report(me, clock)
}

fn propose(replica: &mut Agreement, change: Change) -> bool {
    let me = replica.me;

    replica.propose_change(&identity_of(me), change)
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
        (0..4usize, 0..3usize).prop_map(|(replica, change)| Step::Reconfigure { replica, change }),
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
    in_flight: Vec<(usize, Sent)>,
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
                // Only someone who was ever a contributor has a say.
                let agreement = &mut self.replicas[replica];
                if !agreement.may_speak(agreement.me) {
                    return;
                }

                self.reported[replica] += by;
                let acknowledged = agreement.learned().configuration.clone();
                let report = report(agreement, self.reported[replica]);
                self.proposed.progress.join(&report);
                self.proposed
                    .acknowledged
                    .join(&BTreeMap::from([(agreement.me, acknowledged)]));
            }

            Step::Reconfigure {
                replica,
                change: index,
            } => {
                let agreement = &mut self.replicas[replica];
                let proposal = agreement.learned().configuration.with(change(index));
                if propose(agreement, change(index)) {
                    self.proposed.configuration.join(&proposal);
                }
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
            let messages: Vec<Sent> = self.replicas.iter_mut().map(gossip).collect();
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
    report(&mut phone_replica, 1);

    assert_eq!(phone_replica.try_learn(), None);
}

#[test]
fn two_of_three_members_can_learn_while_one_is_offline() {
    let mut phone_replica = Agreement::new(&genesis(), phone());
    let mut laptop_replica = Agreement::new(&genesis(), laptop());
    let report = report(&mut phone_replica, 1);

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
    propose(&mut laptop_replica, Change::Remove(nas()));
    propose(&mut laptop_replica, Change::Remove(phone()));

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
    let mut phone_replica = Agreement::new(&genesis(), phone());
    let mut laptop_replica = Agreement::new(&genesis(), laptop());
    let mut nas_replica = Agreement::new(&genesis(), nas());

    report(&mut phone_replica, 1);
    send(&mut phone_replica, &mut laptop_replica);
    send(&mut laptop_replica, &mut phone_replica);
    let phone_learned = phone_replica.try_learn().cloned();

    let amnesiac = Agreement::new(&genesis(), laptop());
    laptop_replica = amnesiac;

    report(&mut nas_replica, 1);
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
    propose(&mut phone_replica, Change::Add(friend()));
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
    let report = report(&mut phone_replica, 1);

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
    assert_eq!(
        phone_replica.view().digest(),
        laptop_elsewhere.view().digest()
    );

    send(&mut laptop_elsewhere, &mut phone_replica);
    report(&mut phone_replica, 1);

    assert_eq!(phone_replica.try_learn(), None);
}

/// The friend relays the household's views, and tries to slip
/// something of its own in on the way: an invitation for itself, and
/// a report in the phone's name. Neither is anything a contributor
/// said, so neither is taken.
#[test]
fn a_relay_cant_add_to_what_it_passes_on() {
    let mut phone_replica = Agreement::new(&genesis(), phone());
    let mut laptop_replica = Agreement::new(&genesis(), laptop());
    let mut friend_replica = Agreement::new(&genesis(), friend());
    report(&mut phone_replica, 1);
    send(&mut phone_replica, &mut friend_replica);

    let mut sent = gossip(&mut friend_replica);
    let invitation = Proposal {
        agreement: genesis().id(),
        configuration: genesis().configuration().with(Change::Add(friend())),
    };
    let forged = Report {
        agreement: genesis().id(),
        integrated: VersionVector::from_iter([(phone(), 9)]),
        acknowledged: genesis().configuration(),
    };
    let invitation = Signed::sign(&identity_of(friend()), invitation);
    let forged = Signed::sign(&identity_of(friend()), forged);
    sent.gossip.proposals.push(invitation.signed().clone());
    sent.gossip.reports.push(forged.signed().clone());
    deliver(&mut laptop_replica, sent);

    assert_eq!(laptop_replica.view(), phone_replica.view());
    assert!(!propose(&mut friend_replica, Change::Add(friend())));
}
