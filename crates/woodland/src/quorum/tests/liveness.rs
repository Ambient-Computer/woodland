//! Once the network heals, everyone commits whatever someone proposed.
//!
//! Safety holds on any schedule. Progress can't: a network that loses
//! everything decides nothing. So these schedules are followed by healing,
//! when every node keeps advertising and nothing more is lost.

use proptest::prelude::*;

use super::{Event, Kind, Network, Peek};

fn event() -> impl Strategy<Value = Event> {
    prop_oneof![
        1 => (0..3_usize).prop_map(Event::Propose),
        6 => any::<prop::sample::Index>().prop_map(Event::Deliver),
        1 => any::<prop::sample::Index>().prop_map(Event::Drop),
        1 => any::<prop::sample::Index>().prop_map(Event::Duplicate),
        1 => (0..3_usize).prop_map(Event::Restart),
        1 => (0..3_usize).prop_map(Event::Advertise),
    ]
}

proptest! {
    #[test]
    fn commit_once_the_network_heals(schedule in prop::collection::vec(event(), 0..300)) {
        let mut network = Network::new();
        network.run(&schedule);
        network.heal();

        let proposed = !network.commits.is_empty()
            || network.nodes.iter().any(|node| node.proposer.is_some());

        if proposed {
            for node in &network.nodes {
                prop_assert_eq!(node.ledger.epoch(), 1);
            }
        }
    }
}

// Nobody waits on the highest ballot. Whoever it outbid retries above
// it, so the epoch commits without its proposer.
#[test]
fn commits_when_the_winner_never_returns() {
    let mut network = Network::new();

    let mut order = [0, 1, 2];
    order.sort_by_key(|&node| network.nodes[node].actor.key());
    let [low, mid, high] = order;

    let keys: Vec<_> = network.nodes.iter().map(|node| node.actor.key()).collect();
    let prepare = |proposer: usize| {
        let key = keys[proposer];
        move |peek: &Peek| peek.kind == Kind::Prepare && peek.ballot.proposer == key
    };

    // `low` and `mid` promise `high`, then it disappears for good.
    network.propose(high);
    network.deliver_first(low, prepare(high));
    network.deliver_first(mid, prepare(high));
    network.vanish(high);

    // `low` proposes, gets outbid, and retries.
    network.propose(low);
    network.heal();

    for node in [low, mid] {
        assert_eq!(network.nodes[node].ledger.epoch(), 1);
    }
}
