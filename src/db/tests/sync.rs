use super::*;

#[test]
fn a_reader_follows_along() {
    let (mut sim, group) = Sim::group(2, 1);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.run();

    assert_eq!(
        sim.shown(LAPTOP, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
    assert_eq!(sim.committed(LAPTOP, group, notes(group), "todo"), None);

    sim.settle();

    assert_eq!(
        sim.committed(LAPTOP, group, notes(group), "todo")
            .as_deref(),
        Some("milk")
    );
}

#[test]
fn a_device_that_joins_late_catches_up() {
    let (mut sim, group) = Sim::group(2, 1);
    sim.isolate(LAPTOP);
    for round in 0..200 {
        sim.write_to(
            PHONE,
            group,
            set(notes(group), &format!("note {round}"), "written"),
        );
        if round % 50 == 0 {
            sim.settle();
        }
    }

    sim.connect(PHONE, LAPTOP);
    sim.run();

    let all = Query::prefix(group, notes(group), b"note ");
    assert_eq!(sim.read(LAPTOP, &all).len(), 200);
    assert_eq!(sim.read(LAPTOP, &all), sim.read(PHONE, &all));
    assert_eq!(
        sim.read(LAPTOP, &all.clone().committed()),
        sim.read(PHONE, &all.committed())
    );
}

#[test]
fn devices_that_never_meet_converge_through_one_that_cant_write() {
    let (mut sim, group) = Sim::group(3, 2);
    let relay = NAS;
    sim.disconnect(PHONE, LAPTOP);

    sim.write_to(PHONE, group, set(notes(group), "from phone", "hello"));
    sim.write_to(LAPTOP, group, set(notes(group), "from laptop", "hi"));
    sim.settle();

    for device in [PHONE, LAPTOP, relay] {
        assert_eq!(
            sim.committed(device, group, notes(group), "from phone")
                .as_deref(),
            Some("hello")
        );
        assert_eq!(
            sim.committed(device, group, notes(group), "from laptop")
                .as_deref(),
            Some("hi")
        );
    }
}

#[test]
fn concurrent_writes_to_one_key_end_up_the_same_everywhere() {
    let (mut sim, group) = Sim::group(2, 2);
    sim.disconnect(PHONE, LAPTOP);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.write_to(LAPTOP, group, set(notes(group), "todo", "eggs"));

    sim.connect(PHONE, LAPTOP);
    sim.run();

    let shown = sim.shown(PHONE, group, notes(group), "todo");
    assert!(shown.is_some());
    assert_eq!(sim.shown(LAPTOP, group, notes(group), "todo"), shown);

    sim.settle();

    assert_eq!(sim.committed(PHONE, group, notes(group), "todo"), shown);
    assert_eq!(sim.committed(LAPTOP, group, notes(group), "todo"), shown);
}

#[test]
fn of_two_concurrent_claims_one_happens_everywhere() {
    let (mut sim, group) = Sim::group(2, 2);
    sim.disconnect(PHONE, LAPTOP);
    let phone = Transaction::new()
        .claim(names(group), "alice", "phone")
        .set(notes(group), "winner", "phone");
    let laptop = Transaction::new()
        .claim(names(group), "alice", "laptop")
        .set(notes(group), "winner", "laptop");
    sim.write_to(PHONE, group, phone);
    sim.write_to(LAPTOP, group, laptop);

    assert_eq!(
        sim.shown(PHONE, group, names(group), "alice").as_deref(),
        Some("phone")
    );
    assert_eq!(
        sim.shown(LAPTOP, group, names(group), "alice").as_deref(),
        Some("laptop")
    );

    sim.connect(PHONE, LAPTOP);
    sim.run();

    let holder = sim.shown(PHONE, group, names(group), "alice");
    for device in [PHONE, LAPTOP] {
        assert_eq!(sim.shown(device, group, names(group), "alice"), holder);
        assert_eq!(
            sim.shown(device, group, notes(group), "winner"),
            holder,
            "the loser's other writes went with it"
        );
    }

    sim.settle();

    for device in [PHONE, LAPTOP] {
        assert_eq!(sim.committed(device, group, names(group), "alice"), holder);
        assert_eq!(sim.committed(device, group, notes(group), "winner"), holder);
    }
}

#[test]
fn a_message_from_another_group_or_from_nowhere_is_ignored() {
    let (mut sim, group) = Sim::group(2, 1);
    let (mut other, _) = Sim::group(1, 1);
    let stranger = other.id(PHONE);
    let laptop = sim.id(LAPTOP);
    let greeting = block_on(other.devices[PHONE].connected(laptop)).expect("connected");

    for (_, message) in greeting.sends {
        let outputs = block_on(sim.devices[LAPTOP].receive(stranger, &message)).expect("received");
        assert_eq!(outputs, Outputs::default());
    }
    let outputs = block_on(sim.devices[LAPTOP].receive(stranger, b"nonsense")).expect("received");

    assert_eq!(outputs, Outputs::default());
    assert_eq!(sim.shown(LAPTOP, group, notes(group), "todo"), None);
}
