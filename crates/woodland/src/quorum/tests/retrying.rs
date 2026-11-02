//! A proposer that's outbid retries above the winner, never under a ballot
//! it already used.
//!
//! Every proposer starts at round 1. One that forgot it was outbid and
//! started over would reuse its ballot, possibly for a different operation,
//! and two operations under one ballot can both win. Random schedules
//! rarely find this, so the interleaving is spelled out here.

use super::{Kind, Network, Peek};

#[test]
fn an_outbid_proposer_never_starts_over() {
    let mut network = Network::new();

    // Name the nodes by ballot order: `low` < `mid` < `high`.
    let mut order = [0, 1, 2];
    order.sort_by_key(|&node| network.nodes[node].actor.key());
    let [low, mid, high] = order;

    // Copied out, so the matchers don't borrow the network.
    let keys: Vec<_> = network.nodes.iter().map(|node| node.actor.key()).collect();
    let operations: Vec<_> = (0..network.nodes.len())
        .map(|node| network.wants(node))
        .collect();

    let from = |kind: Kind, proposer: usize| {
        let key = keys[proposer];
        move |peek: &Peek| peek.kind == kind && peek.ballot.proposer == key
    };
    let wants = |proposer: usize, operation: usize| {
        let key = keys[proposer];
        let operation = operations[operation].clone();
        move |peek: &Peek| {
            peek.kind == Kind::Accept
                && peek.ballot.proposer == key
                && peek.operation.as_ref() == Some(&operation)
        }
    };

    // `low` gets its operation accepted by one acceptor: itself.
    network.propose(low);
    network.deliver_first(low, from(Kind::Prepare, low));
    network.deliver_first(mid, from(Kind::Prepare, low));
    network.deliver_first(low, from(Kind::Promise, low));
    network.deliver_first(low, from(Kind::Promise, low));
    network.deliver_first(low, from(Kind::Accept, low));

    // `mid` reaches a majority and asks for its own operation.
    network.propose(mid);
    network.deliver_first(mid, from(Kind::Prepare, mid));
    network.deliver_first(high, from(Kind::Prepare, mid));
    network.deliver_first(mid, from(Kind::Promise, mid));
    network.deliver_first(mid, from(Kind::Promise, mid));

    // `high` outbids it, and `mid` retries above it.
    network.propose(high);
    network.deliver_first(high, from(Kind::Prepare, high));
    network.deliver_first(high, wants(mid, mid));
    network.deliver_first(mid, from(Kind::Rejected, mid));

    // Starting over would reuse `mid`'s first ballot. It would adopt
    // `low`'s operation from `low`'s promise, putting two operations under
    // one ballot. It must not ask for anything new.
    let in_flight = network.in_flight.len();
    network.propose(mid);

    assert_eq!(network.in_flight.len(), in_flight);
    assert_eq!(
        network.nodes[mid].proposer.as_ref().map(|p| p.ballot.round),
        Some(2)
    );

    // Whatever arrives from here, agreement holds.
    network.settle();
}
