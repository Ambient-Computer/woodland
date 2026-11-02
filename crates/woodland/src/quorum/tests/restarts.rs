//! Contributors still agree when nodes restart, losing everything their
//! roles don't say to persist.
//!
//! Restarted nodes only make progress by advertising again, so schedules
//! here resend as well as restart.

use proptest::prelude::*;

use super::{Event, Network};

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
    fn agree_despite_restarts(schedule in prop::collection::vec(event(), 0..300)) {
        let mut network = Network::new();
        network.run(&schedule);
        network.settle();
    }
}
