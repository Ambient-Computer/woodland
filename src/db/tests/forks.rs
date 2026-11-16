//! A contributor that signs two entries for one place in its chain.
//!
//! The honest way to do it is to lose a store and carry on under the
//! same key, which is why a device that loses its store is meant to
//! come back as someone new.

use super::*;

fn refused(sim: &mut Sim, device: usize, group: GroupId, transaction: Transaction) -> bool {
    let written = block_on(sim.devices[device].write(group, transaction));

    matches!(written, Err(Error::NotAContributor))
}

/// The NAS writes something only the phone hears, loses its store, and
/// writes something else that only the laptop hears.
fn forked() -> (Sim, GroupId) {
    let (mut sim, group) = Sim::group(3, 3);
    sim.disconnect_all();
    sim.connect(NAS, PHONE);
    sim.write_to(NAS, group, set(notes(group), "from the nas", "before"));
    sim.run();

    sim.forget(NAS, group);
    sim.connect(NAS, LAPTOP);
    sim.run();
    sim.write_to(NAS, group, set(notes(group), "from the nas", "after"));
    sim.run();

    (sim, group)
}

#[test]
fn a_fork_is_noticed_and_its_author_removed() {
    let (mut sim, group) = forked();
    assert_eq!(
        sim.shown(PHONE, group, notes(group), "from the nas")
            .as_deref(),
        Some("before")
    );
    assert_eq!(
        sim.shown(LAPTOP, group, notes(group), "from the nas")
            .as_deref(),
        Some("after")
    );

    sim.connect(PHONE, LAPTOP);
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(sim.shown(device, group, notes(group), "from the nas"), None);
    }
    assert!(refused(&mut sim, NAS, group, set(notes(group), "x", "y")));

    // The two that are left are a quorum of themselves.
    sim.isolate(NAS);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();
    assert_eq!(
        sim.committed(LAPTOP, group, notes(group), "todo")
            .as_deref(),
        Some("milk")
    );
}

/// Whoever hears of the fork stops taking the author's word at once,
/// before any checkpoint removes it.
#[test]
fn a_proven_author_isnt_listened_to() {
    let (mut sim, group) = forked();
    sim.isolate(NAS);
    sim.connect(PHONE, LAPTOP);
    sim.run();

    sim.write_to(NAS, group, set(notes(group), "more", "from the nas"));
    sim.connect(NAS, LAPTOP);
    sim.run();

    for device in [PHONE, LAPTOP] {
        assert_eq!(sim.shown(device, group, notes(group), "more"), None);
        assert_eq!(sim.shown(device, group, notes(group), "from the nas"), None);
    }

    // The NAS is told, and stops writing.
    assert!(refused(&mut sim, NAS, group, set(notes(group), "x", "y")));
}

/// Of a forked chain, what a checkpoint covers counts. A device holding
/// the other entry lets go of it.
#[test]
fn a_checkpoint_settles_which_of_two_entries_counts() {
    let (mut sim, group) = forked();
    sim.settle();
    assert_eq!(
        sim.committed(LAPTOP, group, notes(group), "from the nas")
            .as_deref(),
        Some("after")
    );

    sim.connect(PHONE, LAPTOP);
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.committed(device, group, notes(group), "from the nas")
                .as_deref(),
            Some("after")
        );
    }
    assert!(refused(&mut sim, NAS, group, set(notes(group), "x", "y")));
}

/// Work written on top of an entry that turns out not to count isn't
/// lost with it.
#[test]
fn honest_work_on_top_of_a_fork_survives_it() {
    let (mut sim, group) = forked();
    sim.write_to(PHONE, group, set(notes(group), "from the phone", "hello"));
    sim.connect(PHONE, LAPTOP);
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.committed(device, group, notes(group), "from the phone")
                .as_deref(),
            Some("hello")
        );
        assert_eq!(sim.shown(device, group, notes(group), "from the nas"), None);
    }
}

#[test]
fn a_proof_outlasts_a_restart() {
    let (mut sim, group) = forked();
    sim.connect(PHONE, LAPTOP);
    sim.run();
    sim.restart(PHONE);
    sim.restart(LAPTOP);

    sim.connect(PHONE, LAPTOP);
    sim.connect(NAS, LAPTOP);
    sim.settle();

    assert!(refused(&mut sim, NAS, group, set(notes(group), "x", "y")));
}

/// A proof travels like anything else: a device that saw neither entry
/// is told by one that saw both.
#[test]
fn a_proof_is_relayed() {
    let (mut sim, group) = Sim::group(4, 4);
    sim.disconnect_all();
    sim.connect(NAS, PHONE);
    sim.write_to(NAS, group, set(notes(group), "from the nas", "before"));
    sim.run();
    sim.forget(NAS, group);
    sim.connect(NAS, LAPTOP);
    sim.run();
    sim.write_to(NAS, group, set(notes(group), "from the nas", "after"));
    sim.run();
    sim.isolate(NAS);

    sim.connect(PHONE, LAPTOP);
    sim.run();
    sim.isolate(PHONE);
    sim.connect(LAPTOP, FRIEND);
    sim.run();

    // The friend knows, having heard it from the laptop alone.
    sim.isolate(LAPTOP);
    sim.connect(NAS, FRIEND);
    sim.write_to(NAS, group, set(notes(group), "more", "from the nas"));
    sim.run();
    assert_eq!(sim.shown(FRIEND, group, notes(group), "more"), None);
}
