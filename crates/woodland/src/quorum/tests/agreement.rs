//! Contributors never commit different operations for the same epoch, no
//! matter how the network reorders, loses, or duplicates messages.

use proptest::prelude::*;

use super::{Event, Network};

fn event() -> impl Strategy<Value = Event> {
    prop_oneof![
        1 => (0..4_usize).prop_map(Event::Propose),
        6 => any::<prop::sample::Index>().prop_map(Event::Deliver),
        1 => any::<prop::sample::Index>().prop_map(Event::Drop),
        1 => any::<prop::sample::Index>().prop_map(Event::Duplicate),
    ]
}

proptest! {
    #[test]
    fn agree_despite_the_network(schedule in prop::collection::vec(event(), 0..300)) {
        let mut network = Network::new();
        network.run(&schedule);
    }

    // Whatever happened, delivering what's left must not break agreement
    // either. Most schedules end with messages still in flight.
    #[test]
    fn agree_once_the_network_settles(schedule in prop::collection::vec(event(), 0..300)) {
        let mut network = Network::new();
        network.run(&schedule);
        network.settle();
    }
}
