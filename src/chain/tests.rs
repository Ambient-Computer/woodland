use proptest::prelude::*;

use super::*;
use crate::testing::peers::{friend, identity_of, laptop, nas, phone};

/// The household: the NAS is always on, so it counts double, and it and
/// either other device are a quorum. The phone and laptop alone aren't.
fn household() -> Electorate {
    Electorate::new([(phone(), 1), (laptop(), 1), (nas(), 2)])
}

fn founding() -> Checkpoint {
    Checkpoint::founding([7; 32], household())
}

/// A device and its replica of the chain.
struct Replica {
    peer: Peer,
    chain: Chain,
}

impl Replica {
    fn new(peer: Peer) -> Self {
        Self::founded(peer, founding())
    }

    fn founded(peer: Peer, founding: Checkpoint) -> Self {
        let chain = Chain::new(founding);

        Self { peer, chain }
    }

    fn propose(&mut self, electorate: Electorate, body: &[u8]) -> bool {
        let identity = identity_of(self.peer);
        let proposed = self.chain.propose(&identity, electorate, body.to_vec());
        self.chain.advance(&identity, |_| true);

        proposed
    }

    /// Hears everything `from` holds, and acts on it.
    fn hear(&mut self, from: &Self) -> bool {
        self.deliver(from.chain.gossip(0))
    }

    fn deliver(&mut self, gossip: Gossip) -> bool {
        let grew = self.chain.receive(gossip);

        self.chain.advance(&identity_of(self.peer), |_| true) || grew
    }

    fn height(&self) -> u64 {
        self.chain.tip().height
    }

    fn body(&self) -> &[u8] {
        &self.chain.tip().body
    }

    fn digests(&self) -> Vec<Digest> {
        self.chain.checkpoints().map(Checkpoint::digest).collect()
    }
}

/// Two devices talk until neither has anything new to say.
fn converse(replicas: &mut [Replica], a: usize, b: usize) {
    settle(replicas, &[(a, b), (b, a)]);
}

/// Sends along `links` until nothing changes.
fn settle(replicas: &mut [Replica], links: &[(usize, usize)]) {
    for _ in 0..64 {
        let mut changed = false;
        for &(from, to) in links {
            let gossip = replicas[from].chain.gossip(0);
            changed |= replicas[to].deliver(gossip);
        }
        if !changed {
            return;
        }
    }

    panic!("the replicas never went quiet");
}

/// Everyone hears everyone.
fn mesh(count: usize) -> Vec<(usize, usize)> {
    let pairs = (0..count).flat_map(|from| (0..count).map(move |to| (from, to)));

    pairs.filter(|(from, to)| from != to).collect()
}

const PHONE: usize = 0;
const LAPTOP: usize = 1;
const NAS: usize = 2;
const FRIEND: usize = 3;

fn cast() -> Vec<Replica> {
    [phone(), laptop(), nas(), friend()]
        .map(Replica::new)
        .into()
}

/// The NAS decides alone. The friend writes, and has no say.
fn guests() -> Electorate {
    Electorate::of([(nas(), Role::witness(1)), (friend(), Role::contributor(0))])
}

fn hosted() -> Vec<Replica> {
    let founding = Checkpoint::founding([7; 32], guests());

    [nas(), friend()]
        .map(|peer| Replica::founded(peer, founding.clone()))
        .into()
}

#[test]
fn a_member_with_no_say_can_start_an_attempt() {
    let mut replicas = hosted();

    assert!(replicas[1].propose(guests(), b"from the friend"));
    assert_eq!(replicas[1].height(), 0, "its own word decides nothing");

    converse(&mut replicas, 0, 1);

    for replica in &replicas {
        assert_eq!(replica.height(), 1);
        assert_eq!(replica.body(), b"from the friend");
    }
}

#[test]
fn a_member_with_no_say_cant_give_itself_one() {
    let mut replicas = hosted();
    let coup = Electorate::of([(friend(), Role::contributor(1))]);

    assert!(replicas[1].propose(coup, b"from the friend"));
    converse(&mut replicas, 0, 1);

    for replica in &replicas {
        assert_eq!(replica.height(), 0);
    }
}

#[test]
fn an_electorate_nothing_could_follow_is_never_proposed() {
    let mut replica = Replica::new(nas());
    let nobody = Electorate::new([]);
    let only_witnesses = Electorate::of([(nas(), Role::witness(1))]);
    let no_say = Electorate::of([(nas(), Role::contributor(0))]);

    for dead in [nobody, only_witnesses, no_say] {
        assert!(!replica.propose(dead, b"the end"));
    }
}

#[test]
fn a_founder_alone_decides_by_itself() {
    let alone = Electorate::new([(phone(), 1)]);
    let mut replica = Replica::founded(phone(), Checkpoint::founding([7; 32], alone.clone()));

    assert!(replica.propose(alone, b"first"));

    assert_eq!(replica.height(), 1);
    assert_eq!(replica.body(), b"first");
}

#[test]
fn the_heavy_device_and_any_other_decide() {
    let mut replicas = cast();

    replicas[NAS].propose(household(), b"first");
    converse(&mut replicas, NAS, PHONE);

    assert_eq!(replicas[NAS].height(), 1);
    assert_eq!(replicas[PHONE].height(), 1);
    assert_eq!(replicas[LAPTOP].height(), 0);
}

#[test]
fn the_light_devices_cant_decide_without_the_heavy_one() {
    let mut replicas = cast();

    replicas[PHONE].propose(household(), b"first");
    converse(&mut replicas, PHONE, LAPTOP);

    assert_eq!(replicas[PHONE].height(), 0);
    assert_eq!(replicas[LAPTOP].height(), 0);
}

#[test]
fn a_device_with_no_say_cant_propose() {
    let mut replicas = cast();

    assert!(!replicas[FRIEND].propose(household(), b"mine"));
}

#[test]
fn electors_that_never_meet_decide_through_a_device_with_no_say() {
    let mut replicas = cast();

    replicas[NAS].propose(household(), b"first");
    let chain = [
        (NAS, FRIEND),
        (FRIEND, PHONE),
        (PHONE, FRIEND),
        (FRIEND, NAS),
    ];
    settle(&mut replicas, &chain);

    for replica in [NAS, FRIEND, PHONE] {
        assert_eq!(replicas[replica].height(), 1);
    }
}

#[test]
fn a_device_that_was_away_follows_the_chain_from_one_that_had_no_say() {
    let mut replicas = cast();
    for body in [&b"first"[..], b"second"] {
        replicas[NAS].propose(household(), body);
        settle(
            &mut replicas,
            &[(NAS, FRIEND), (FRIEND, NAS), (NAS, PHONE), (PHONE, NAS)],
        );
    }

    let (laptop, friend) = replicas.split_at_mut(FRIEND);
    laptop[LAPTOP].hear(&friend[0]);

    assert_eq!(replicas[LAPTOP].height(), 2);
    assert_eq!(replicas[LAPTOP].body(), b"second");
}

#[test]
fn rivals_decide_on_one_of_them() {
    let mut replicas = cast();

    replicas[PHONE].propose(household(), b"phone's");
    replicas[LAPTOP].propose(household(), b"laptop's");
    settle(&mut replicas, &mesh(4));

    let decided = replicas[NAS].body().to_vec();
    assert!(decided == b"phone's" || decided == b"laptop's");
    for replica in &replicas {
        assert_eq!(replica.height(), 1);
        assert_eq!(replica.body(), decided);
    }
}

/// The heart of safety. Once a checkpoint might have a quorum's votes, a
/// later attempt can only carry that one, whatever its proposer wanted.
#[test]
fn a_rival_finishes_what_an_elector_already_voted_for() {
    let mut replicas = cast();

    // The NAS gets a quorum's promises and votes, but nobody else does.
    replicas[NAS].propose(household(), b"nas's");
    replicas.swap(PHONE, NAS);
    let (nas, phone) = replicas.split_at_mut(NAS);
    phone[0].hear(&nas[PHONE]);
    nas[PHONE].hear(&phone[0]);
    replicas.swap(PHONE, NAS);
    assert_eq!(replicas[NAS].height(), 0);

    // The laptop outbids it without having heard any of that.
    replicas[LAPTOP].propose(household(), b"laptop's");
    replicas[LAPTOP].propose(household(), b"laptop's");
    converse(&mut replicas, LAPTOP, NAS);

    assert_eq!(replicas[LAPTOP].height(), 1);
    assert_eq!(replicas[LAPTOP].body(), b"nas's");
}

/// The other half of safety. A promise is what lets a later attempt
/// trust what it was told: nothing earlier can still gather votes.
#[test]
fn an_elector_wont_vote_in_an_attempt_it_promised_against() {
    let mut replicas = cast();

    // The NAS proposes with the phone's promise, and votes.
    replicas[NAS].propose(household(), b"nas's");
    settle(&mut replicas, &[(NAS, PHONE)]);
    settle(&mut replicas, &[(PHONE, NAS)]);

    // Before the phone hears of it, the laptop outbids the NAS.
    replicas[LAPTOP].propose(household(), b"laptop's");
    replicas[LAPTOP].propose(household(), b"laptop's");
    settle(&mut replicas, &[(LAPTOP, PHONE)]);
    settle(&mut replicas, &[(NAS, PHONE)]);

    assert_eq!(replicas[PHONE].height(), 0);
}

#[test]
fn a_removed_elector_has_no_say_in_what_follows() {
    let mut replicas = cast();
    let without_nas = Electorate::new([(phone(), 1), (laptop(), 1)]);

    replicas[PHONE].propose(without_nas.clone(), b"removal");
    settle(&mut replicas, &mesh(4));
    assert_eq!(replicas[NAS].height(), 1);

    assert!(!replicas[NAS].propose(household(), b"nas's"));

    replicas[PHONE].propose(without_nas, b"after");
    converse(&mut replicas, PHONE, LAPTOP);
    assert_eq!(replicas[LAPTOP].height(), 2);
}

#[test]
fn a_new_elector_has_a_say_in_what_follows() {
    let mut replicas = cast();
    let friends = Electorate::new([(friend(), 1)]);

    replicas[NAS].propose(friends.clone(), b"handover");
    settle(&mut replicas, &mesh(4));

    assert!(replicas[FRIEND].propose(friends, b"friend's"));
    assert_eq!(replicas[FRIEND].height(), 2);
}

#[test]
fn a_checkpoint_without_a_quorums_votes_is_ignored() {
    let mut replicas = cast();
    replicas[NAS].propose(household(), b"first");
    converse(&mut replicas, NAS, PHONE);

    let mut gossip = replicas[NAS].chain.gossip(0);
    gossip.decided[0]
        .votes
        .retain(|vote| vote.author() != nas());

    assert!(!replicas[LAPTOP].deliver(gossip));
    assert_eq!(replicas[LAPTOP].height(), 0);
}

#[derive(Clone, Copy, Debug)]
enum Step {
    /// A replica starts an attempt at the next checkpoint.
    Propose {
        replica: usize,
        electorate: usize,
        body: u8,
    },
    /// A replica takes over the latest attempt it knows to have named
    /// a checkpoint.
    Finish { replica: usize },
    /// A replica tells another everything it holds, right away.
    Tell { from: usize, to: usize },
    /// A replica sends everything it holds. It may arrive much later.
    Send { from: usize, to: usize },
    /// An in-flight message arrives, and maybe stays in flight to arrive
    /// again (duplication).
    Deliver { message: usize, duplicate: bool },
    /// An in-flight message is lost.
    Drop { message: usize },
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        1 => (0..4usize, 0..5usize, any::<u8>()).prop_map(|(replica, electorate, body)| {
            Step::Propose {
                replica,
                electorate,
                body,
            }
        }),
        1 => (0..4usize).prop_map(|replica| Step::Finish { replica }),
        8 => (0..4usize, 1..4usize).prop_map(|(from, offset)| Step::Tell {
            from,
            to: (from + offset) % 4
        }),
        2 => (0..4usize, 1..4usize).prop_map(|(from, offset)| Step::Send {
            from,
            to: (from + offset) % 4
        }),
        2 => (any::<usize>(), any::<bool>())
            .prop_map(|(message, duplicate)| Step::Deliver { message, duplicate }),
        1 => any::<usize>().prop_map(|message| Step::Drop { message }),
    ]
}

/// The electorates anyone may propose. Never an empty one.
fn electorate(index: usize) -> Electorate {
    [
        household(),
        Electorate::new([(phone(), 1), (laptop(), 1), (nas(), 2), (friend(), 1)]),
        Electorate::new([(phone(), 1), (laptop(), 1)]),
        Electorate::new([(nas(), 1)]),
        Electorate::new([(laptop(), 1), (friend(), 1)]),
    ][index]
        .clone()
}

/// Four replicas and an unreliable network.
struct Simulation {
    replicas: Vec<Replica>,
    /// Each message, with the replica it's addressed to.
    in_flight: Vec<(usize, Gossip)>,
}

impl Simulation {
    fn new() -> Self {
        Self {
            replicas: cast(),
            in_flight: Vec::new(),
        }
    }

    fn run(&mut self, step: Step) {
        match step {
            Step::Propose {
                replica,
                electorate: index,
                body,
            } => {
                self.replicas[replica].propose(electorate(index), &[body]);
            }
            Step::Finish { replica } => {
                let replica = &mut self.replicas[replica];
                let identity = identity_of(replica.peer);
                replica.chain.finish(&identity);
                replica.chain.advance(&identity, |_| true);
            }
            Step::Tell { from, to } => {
                let gossip = self.replicas[from].chain.gossip(0);
                self.replicas[to].deliver(gossip);
            }
            Step::Send { from, to } => {
                let gossip = self.replicas[from].chain.gossip(0);
                self.in_flight.push((to, gossip));
            }
            Step::Deliver { message, duplicate } => {
                if self.in_flight.is_empty() {
                    return;
                }
                let index = message % self.in_flight.len();
                let (to, gossip) = if duplicate {
                    self.in_flight[index].clone()
                } else {
                    self.in_flight.swap_remove(index)
                };
                self.replicas[to].deliver(gossip);
            }
            Step::Drop { message } => {
                if !self.in_flight.is_empty() {
                    let index = message % self.in_flight.len();
                    self.in_flight.swap_remove(index);
                }
            }
        }
    }

    /// Every pair of replicas holds a prefix of one chain.
    fn agrees(&self) -> bool {
        let chains: Vec<_> = self.replicas.iter().map(Replica::digests).collect();

        chains.iter().all(|a| {
            chains
                .iter()
                .all(|b| a.iter().zip(b).all(|(ours, theirs)| ours == theirs))
        })
    }

    fn highest(&self) -> u64 {
        let heights = self.replicas.iter().map(Replica::height);

        heights.max().unwrap_or(0)
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    /// Whatever is proposed, by whoever, and whatever the network does,
    /// no two replicas ever hold different checkpoints at one height.
    #[test]
    fn no_two_checkpoints_are_decided_for_one_slot(
        steps in prop::collection::vec(step(), 0..160),
    ) {
        let mut simulation = Simulation::new();
        for step in steps {
            simulation.run(step);
            prop_assert!(simulation.agrees());
        }
    }

    /// However tangled things got, once everyone can hear everyone and
    /// one elector proposes, the chain grows.
    #[test]
    fn a_connected_quorum_decides_once_rivals_stand_back(
        steps in prop::collection::vec(step(), 0..120),
    ) {
        let mut simulation = Simulation::new();
        for step in steps {
            simulation.run(step);
        }
        settle(&mut simulation.replicas, &mesh(4));
        let height = simulation.highest();
        prop_assert!(simulation.replicas.iter().all(|it| it.height() == height));

        let next = simulation.replicas[0].chain.tip().electorate.clone();
        let proposed = simulation
            .replicas
            .iter_mut()
            .any(|it| it.propose(next.clone(), b"healed"));
        prop_assert!(proposed);
        settle(&mut simulation.replicas, &mesh(4));

        prop_assert!(simulation.agrees());
        prop_assert!(simulation.replicas.iter().all(|it| it.height() == height + 1));
    }
}

/// The phone writes, two witnesses decide with it, and the friend
/// writes with no say.
fn witnessed() -> Vec<Replica> {
    let electorate = Electorate::of([
        (phone(), Role::contributor(1)),
        (laptop(), Role::witness(1)),
        (nas(), Role::witness(1)),
        (friend(), Role::contributor(0)),
    ]);
    let founding = Checkpoint::founding([7; 32], electorate);
    let replica = |peer| Replica::founded(peer, founding.clone());

    [phone(), laptop(), nas(), friend()].map(replica).into()
}

/// The phone names a checkpoint, and both witnesses promise a later
/// attempt before they hear of it. That attempt's author vanishes
/// without naming anything, and so does the phone.
fn stranded() -> Vec<Replica> {
    let mut cast = witnessed();
    let electorate = cast[PHONE].chain.tip().electorate.clone();
    cast[PHONE].propose(electorate.clone(), b"a");
    let prepare = cast[PHONE].chain.gossip(0);
    cast[NAS].deliver(prepare);
    let promise = cast[NAS].chain.gossip(0);
    cast[PHONE].deliver(promise);
    assert_eq!(cast[PHONE].chain.proposed().count(), 1);

    let named = cast[PHONE].chain.gossip(0);
    cast[FRIEND].deliver(named.clone());
    cast[FRIEND].propose(electorate, b"b");
    let rival = cast[FRIEND].chain.gossip(0);
    cast[LAPTOP].deliver(rival.clone());
    cast[NAS].deliver(rival);
    converse(&mut cast, LAPTOP, NAS);
    assert_eq!(cast[LAPTOP].height(), 0);
    assert_eq!(cast[NAS].height(), 0);

    cast
}

#[test]
fn a_witness_finishes_an_attempt_whose_author_is_gone() {
    let mut cast = stranded();

    let identity = identity_of(laptop());
    assert!(cast[LAPTOP].chain.finish(&identity));
    cast[LAPTOP].chain.advance(&identity, |_| true);
    converse(&mut cast, LAPTOP, NAS);

    for witness in [LAPTOP, NAS] {
        assert_eq!(cast[witness].height(), 1);
        assert_eq!(cast[witness].body(), b"a");
    }

    // The phone comes back to find what it proposed decided.
    converse(&mut cast, PHONE, NAS);
    assert_eq!(cast[PHONE].body(), b"a");
}

#[test]
fn theres_nothing_to_finish_before_anything_is_named() {
    let mut cast = witnessed();
    let electorate = cast[PHONE].chain.tip().electorate.clone();
    cast[PHONE].propose(electorate, b"a");
    let prepare = cast[PHONE].chain.gossip(0);
    cast[LAPTOP].deliver(prepare);

    assert!(!cast[LAPTOP].chain.finish(&identity_of(laptop())));
}

#[test]
fn only_a_member_finishes_an_attempt() {
    let mut cast = stranded();
    let outsider = Identity::from_secret_key([99; 32]);

    assert!(!cast[LAPTOP].chain.finish(&outsider));
}
