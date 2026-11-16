use super::*;

#[test]
fn a_new_contributor_writes_once_a_checkpoint_admits_it() {
    let (mut sim, group) = Sim::group(2, 1);
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);

    let early = block_on(sim.devices[LAPTOP].write(group, set(notes(group), "todo", "milk")));
    assert!(matches!(early, Err(Error::NotAContributor)));

    sim.settle();
    sim.write_to(LAPTOP, group, set(notes(group), "todo", "milk"));
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

    sim.write_to(LAPTOP, group, set(notes(group), "todo", "milk"));
    sim.run();

    assert_eq!(
        sim.shown(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
}

#[test]
fn a_removed_contributor_loses_what_it_wrote_after_its_last_checkpoint() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.write_to(NAS, group, set(notes(group), "kept", "committed"));
    sim.settle();
    sim.isolate(NAS);
    sim.write_to(NAS, group, set(notes(group), "lost", "never committed"));

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
    sim.write_to(NAS, group, set(notes(group), "from nas", "uncommitted"));
    sim.run();
    sim.write_to(
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
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
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

    sim.write_to(LAPTOP, group, set(notes(group), "todo", "milk"));
    sim.settle();

    assert_eq!(
        sim.committed(LAPTOP, group, notes(group), "todo")
            .as_deref(),
        Some("milk")
    );
}

/// Found by the randomized simulation: a contributor removed and added
/// back, with something written before either that nobody had heard.
#[test]
fn coming_back_doesnt_bring_back_what_removal_cut() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    sim.write_to(NAS, group, set(notes(group), "lost", "before removal"));

    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP, NAS]);
    sim.settle();
    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(sim.shown(device, group, notes(group), "lost"), None);
    }

    sim.write_to(NAS, group, set(notes(group), "kept", "after coming back"));
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        let kept = sim.committed(device, group, notes(group), "kept");
        assert_eq!(kept.as_deref(), Some("after coming back"));
    }
}

/// The laptop writes on top of something the friend wrote, and the
/// friend is removed before anyone else has seen either. What the
/// friend wrote is gone for good. What the laptop wrote isn't, and
/// letting the friend back in doesn't leave it waiting for an entry
/// that will never come.
#[test]
fn work_on_top_of_a_cut_entry_counts_even_once_its_author_is_back() {
    let (mut sim, group) = Sim::group(4, 4);
    let everyone = |it: usize| (it, Role::contributor(u32::from(it != FRIEND)));
    sim.assign(PHONE, group, (0..4).map(everyone));
    sim.settle();
    sim.disconnect_all();

    sim.connect(FRIEND, LAPTOP);
    write(&mut sim, FRIEND, "guest", "was here");
    sim.run();
    write(&mut sim, LAPTOP, "host", "saw that");
    sim.disconnect_all();

    // The phone and the NAS remove the friend, and let it back in.
    sim.connect(PHONE, NAS);
    sim.assign(PHONE, group, (0..3).map(everyone));
    sim.settle();
    sim.assign(PHONE, group, (0..4).map(everyone));
    sim.settle();

    sim.connect_all();
    sim.settle();

    let model = household_notes(&sim);
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let host = sim.committed(device, group, model, "host");
        assert_eq!(host.as_deref(), Some("saw that"), "device {device}");
        assert_eq!(sim.committed(device, group, model, "guest"), None);
        assert_eq!(sim.shown(device, group, model, "guest"), None);
    }
}
