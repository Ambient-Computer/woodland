//! Agreement holds across many epochs, while membership keeps changing.
//!
//! Each commit changes who votes next, so this is where removed nodes,
//! changing majorities, and nodes from different epochs meet. Offline-first
//! networks come and go, so each schedule alternates bad stretches with
//! healing. Without the healing, few schedules get past a couple of epochs.

use proptest::prelude::*;

use super::{Event, Network};

fn event() -> impl Strategy<Value = Event> {
    prop_oneof![
        2 => (0..4_usize).prop_map(Event::Propose),
        6 => any::<prop::sample::Index>().prop_map(Event::Deliver),
        1 => any::<prop::sample::Index>().prop_map(Event::Drop),
        1 => any::<prop::sample::Index>().prop_map(Event::Duplicate),
        1 => (0..4_usize).prop_map(Event::Restart),
        1 => (0..4_usize).prop_map(Event::Advertise),
    ]
}

proptest! {
    #[test]
    fn agree_while_membership_changes(
        stretches in prop::collection::vec(prop::collection::vec(event(), 0..60), 0..12),
    ) {
        let mut network = Network::new();
        for stretch in &stretches {
            network.run(stretch);
            network.heal();

            let epoch = network.nodes[0].ledger.epoch();
            for node in &network.nodes {
                prop_assert_eq!(node.ledger.epoch(), epoch);
                prop_assert!(node.proposer.is_none());
            }
        }
    }
}
