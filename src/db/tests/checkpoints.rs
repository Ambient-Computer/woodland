use super::*;

#[test]
fn nothing_commits_without_a_quorum() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(PHONE);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();

    assert_eq!(
        sim.shown(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
    assert_eq!(sim.committed(PHONE, group, notes(group), "todo"), None);

    sim.isolate(NAS);
    sim.connect(PHONE, LAPTOP);
    sim.settle();

    assert_eq!(
        sim.committed(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
    assert_eq!(
        sim.committed(LAPTOP, group, notes(group), "todo")
            .as_deref(),
        Some("milk")
    );
    assert_eq!(sim.committed(NAS, group, notes(group), "todo"), None);
}

#[test]
fn a_quorum_commits_while_a_contributor_is_away() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();

    assert_eq!(
        sim.committed(LAPTOP, group, notes(group), "todo")
            .as_deref(),
        Some("milk")
    );

    sim.connect(NAS, LAPTOP);
    sim.run();

    assert_eq!(
        sim.committed(NAS, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
}

#[test]
fn committed_work_isnt_undone_by_work_that_arrives_late() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);

    // The NAS claims first, by any clock. Nobody hears.
    sim.write_to(NAS, group, claim(names(group), "alice", "nas"));
    for round in 0..5 {
        sim.write_to(PHONE, group, set(notes(group), "busy", &round.to_string()));
    }
    sim.write_to(PHONE, group, claim(names(group), "alice", "phone"));
    sim.settle();

    assert_eq!(
        sim.committed(LAPTOP, group, names(group), "alice")
            .as_deref(),
        Some("phone")
    );
    assert_eq!(
        sim.shown(NAS, group, names(group), "alice").as_deref(),
        Some("nas")
    );

    sim.connect(NAS, PHONE);
    sim.connect(NAS, LAPTOP);
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.shown(device, group, names(group), "alice").as_deref(),
            Some("phone")
        );
        assert_eq!(
            sim.committed(device, group, names(group), "alice")
                .as_deref(),
            Some("phone")
        );
    }
}

#[test]
fn late_work_that_conflicts_with_nothing_still_happens() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    sim.write_to(NAS, group, set(notes(group), "from nas", "hello"));
    sim.write_to(PHONE, group, set(notes(group), "from phone", "hi"));
    sim.settle();

    sim.connect(NAS, PHONE);
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.committed(device, group, notes(group), "from nas")
                .as_deref(),
            Some("hello")
        );
        assert_eq!(
            sim.committed(device, group, notes(group), "from phone")
                .as_deref(),
            Some("hi")
        );
    }
}

#[test]
fn a_device_restarted_in_the_middle_of_an_attempt_still_agrees() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.run();
    sim.tick(PHONE);
    for _ in 0..3 {
        sim.deliver();
    }

    sim.restart(LAPTOP);
    sim.restart(PHONE);
    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.committed(device, group, notes(group), "todo")
                .as_deref(),
            Some("milk")
        );
    }
}

#[test]
fn rival_attempts_end_in_one_checkpoint() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.disconnect(PHONE, LAPTOP);
    sim.write_to(PHONE, group, claim(names(group), "alice", "phone"));
    sim.write_to(LAPTOP, group, claim(names(group), "alice", "laptop"));
    sim.tick(PHONE);
    sim.tick(LAPTOP);
    sim.connect(PHONE, LAPTOP);
    sim.settle();

    let holder = sim.committed(NAS, group, names(group), "alice");
    assert!(holder.is_some());
    for device in [PHONE, LAPTOP] {
        assert_eq!(sim.committed(device, group, names(group), "alice"), holder);
    }
}

/// A threshold model shows only what's committed, so nobody acts on a
/// value that could still be reordered.
#[test]
fn a_threshold_value_shows_once_its_committed() {
    let (mut sim, group) = Sim::group(3, 3);
    let limits = ModelId::threshold(group, [4; 32]);
    sim.isolate(PHONE);
    sim.write_to(PHONE, group, set(limits, "quota", "10"));
    sim.settle();

    assert_eq!(sim.shown(PHONE, group, limits, "quota"), None);

    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.shown(device, group, limits, "quota").as_deref(),
            Some("10")
        );
    }
}

/// A vote says "I reached this state too". A contributor that didn't,
/// for whatever reason, doesn't vote, and without it there's no quorum.
#[test]
fn a_contributor_that_reaches_another_state_doesnt_vote() {
    let (mut sim, group) = Sim::group(2, 2);
    let corrupt = Write::Put {
        key: keys::state(group, group.into()),
        value: vec![7; 2048],
    };
    block_on(sim.devices[LAPTOP].store.apply(vec![corrupt])).expect("applied");

    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();

    assert_eq!(
        sim.shown(LAPTOP, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
    for device in [PHONE, LAPTOP] {
        assert_eq!(sim.committed(device, group, notes(group), "todo"), None);
    }
}
