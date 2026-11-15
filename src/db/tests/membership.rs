use super::*;

#[test]
fn a_new_contributor_writes_once_a_checkpoint_admits_it() {
    let (mut sim, group) = Sim::group(2, 1);
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);

    let early = block_on(sim.devices[LAPTOP].write(group, set(notes(group), "todo", "milk")));
    assert!(matches!(early, Err(Error::NotAContributor)));

    sim.settle();
    sim.write(LAPTOP, group, set(notes(group), "todo", "milk"));
    sim.settle();

    assert_eq!(
        sim.committed(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
}

#[test]
fn a_request_to_reconfigure_survives_a_restart() {
    let (mut sim, group) = Sim::group(2, 1);
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.restart(PHONE);
    sim.connect_all();
    sim.settle();

    sim.write(LAPTOP, group, set(notes(group), "todo", "milk"));
    sim.run();

    assert_eq!(
        sim.shown(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
}

#[test]
fn a_removed_contributor_loses_what_it_wrote_after_its_last_checkpoint() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.write(NAS, group, set(notes(group), "kept", "committed"));
    sim.settle();
    sim.isolate(NAS);
    sim.write(NAS, group, set(notes(group), "lost", "never committed"));

    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();
    sim.connect(NAS, PHONE);
    sim.connect(NAS, LAPTOP);
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.committed(device, group, notes(group), "kept")
                .as_deref(),
            Some("committed")
        );
        assert_eq!(sim.shown(device, group, notes(group), "lost"), None);
    }
    let refused = block_on(sim.devices[NAS].write(group, set(notes(group), "more", "?")));
    assert!(matches!(refused, Err(Error::NotAContributor)));
}

#[test]
fn work_written_on_top_of_a_removed_contributors_survives() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.disconnect(NAS, PHONE);
    sim.write(NAS, group, set(notes(group), "from nas", "uncommitted"));
    sim.run();
    sim.write(
        LAPTOP,
        group,
        set(notes(group), "from laptop", "after the nas"),
    );
    sim.isolate(NAS);

    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();
    sim.settle();

    for device in [PHONE, LAPTOP] {
        assert_eq!(
            sim.committed(device, group, notes(group), "from laptop")
                .as_deref(),
            Some("after the nas")
        );
    }
}

#[test]
fn a_removed_contributor_has_no_say() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();

    sim.isolate(LAPTOP);
    sim.write(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();

    assert_eq!(
        sim.committed(PHONE, group, notes(group), "todo"),
        None,
        "the phone and the NAS aren't a quorum of two"
    );
}

#[test]
fn a_group_can_shrink_to_one_and_carry_on() {
    let (mut sim, group) = Sim::group(2, 2);
    sim.reconfigure(LAPTOP, group, [LAPTOP]);
    sim.settle();
    sim.isolate(LAPTOP);

    sim.write(LAPTOP, group, set(notes(group), "todo", "milk"));
    sim.settle();

    assert_eq!(
        sim.committed(LAPTOP, group, notes(group), "todo")
            .as_deref(),
        Some("milk")
    );
}
