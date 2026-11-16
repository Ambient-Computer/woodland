//! What a role lets a device do: write everywhere, write to some
//! clocks, or only count.

use super::*;

fn photos(group: GroupId) -> ClockId {
    ClockId::new(group, [7; 32])
}

fn captions(group: GroupId) -> ModelId {
    ModelId::kv(photos(group), [1; 32])
}

fn refused(sim: &mut Sim, device: usize, group: GroupId, transaction: Transaction) -> bool {
    let written = block_on(sim.devices[device].write(group, transaction));

    matches!(written, Err(Error::NotAContributor))
}

/// The phone and the laptop write. The NAS only counts.
fn witnessed() -> (Sim, GroupId) {
    let (mut sim, group) = Sim::group(3, 3);
    let roles = [
        (PHONE, Role::contributor(1)),
        (LAPTOP, Role::contributor(1)),
        (NAS, Role::witness(1)),
    ];
    sim.assign(PHONE, group, roles);
    sim.settle();

    (sim, group)
}

#[test]
fn a_witness_counts_and_never_writes() {
    let (mut sim, group) = witnessed();
    assert!(refused(&mut sim, NAS, group, set(notes(group), "x", "y")));

    sim.isolate(LAPTOP);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();

    assert_eq!(
        sim.committed(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
}

/// A witness has nothing to check a checkpoint against, so it starts
/// none. It finishes what contributors start.
#[test]
fn witnesses_alone_commit_nothing_new() {
    let (mut sim, group) = Sim::group(3, 3);
    let roles = [
        (PHONE, Role::contributor(1)),
        (LAPTOP, Role::witness(1)),
        (NAS, Role::witness(1)),
    ];
    sim.assign(PHONE, group, roles);
    sim.settle();

    sim.isolate(PHONE);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();
    assert_eq!(sim.committed(PHONE, group, notes(group), "todo"), None);

    sim.connect(PHONE, NAS);
    sim.settle();

    for device in [PHONE, NAS] {
        assert_eq!(
            sim.committed(device, group, notes(group), "todo")
                .as_deref(),
            Some("milk")
        );
    }
}

#[test]
fn a_contributor_writes_only_to_the_clocks_its_role_lists() {
    let (mut sim, group) = Sim::group(3, 3);
    let roles = [
        (PHONE, Role::contributor(1)),
        (LAPTOP, Role::contributor(1)),
        (NAS, Role::contributor(1).only([photos(group)])),
    ];
    sim.assign(PHONE, group, roles);
    sim.settle();

    assert!(refused(&mut sim, NAS, group, set(notes(group), "x", "y")));
    sim.write_to(NAS, group, set(captions(group), "beach", "sunset"));
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.committed(device, group, captions(group), "beach")
                .as_deref(),
            Some("sunset")
        );
    }
}

/// Narrowing a contributor is removing it from the clocks it loses:
/// what it wrote there and nobody committed is gone. What it wrote where
/// it may still write stands.
#[test]
fn narrowing_a_contributor_cuts_it_from_the_clocks_it_loses() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    sim.write_to(NAS, group, set(notes(group), "todo", "from the nas"));
    sim.write_to(NAS, group, set(captions(group), "beach", "from the nas"));
    let roles = [
        (PHONE, Role::contributor(1)),
        (LAPTOP, Role::contributor(1)),
        (NAS, Role::contributor(1).only([photos(group)])),
    ];
    sim.assign(PHONE, group, roles);
    sim.settle();

    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(sim.shown(device, group, notes(group), "todo"), None);
        assert_eq!(
            sim.committed(device, group, captions(group), "beach")
                .as_deref(),
            Some("from the nas")
        );
    }
}

/// Weight and writing are separate. A contributor with no weight is
/// never waited for, and never outvotes anyone.
#[test]
fn a_contributor_with_no_weight_writes_and_has_no_say() {
    let (mut sim, group) = Sim::group(3, 3);
    let roles = [
        (PHONE, Role::contributor(1)),
        (LAPTOP, Role::contributor(0)),
        (NAS, Role::contributor(0)),
    ];
    sim.assign(PHONE, group, roles);
    sim.settle();

    sim.isolate(PHONE);
    sim.write_to(LAPTOP, group, set(notes(group), "todo", "milk"));
    sim.settle();
    assert_eq!(sim.committed(NAS, group, notes(group), "todo"), None);

    sim.connect(PHONE, NAS);
    sim.settle();

    for device in [PHONE, NAS] {
        assert_eq!(
            sim.committed(device, group, notes(group), "todo")
                .as_deref(),
            Some("milk")
        );
    }

    let outvote = [(sim.id(LAPTOP), Role::contributor(9))];
    let asked = block_on(sim.devices[LAPTOP].reconfigure(group, outvote));
    assert!(matches!(asked, Err(Error::NotAContributor)));
}

/// Whoever can work out what a checkpoint commits can start one, with
/// a say or without. Those with a say decide.
#[test]
fn a_writer_with_no_say_has_its_work_committed_by_those_with_one() {
    let (mut sim, group) = Sim::group(2, 2);
    let roles = [
        (PHONE, Role::witness(1)),
        (LAPTOP, Role::contributor(0).only([photos(group)])),
    ];
    sim.assign(PHONE, group, roles);
    sim.settle();

    sim.write_to(LAPTOP, group, set(captions(group), "beach", "sunset"));
    sim.settle();

    for device in [PHONE, LAPTOP] {
        assert_eq!(
            sim.committed(device, group, captions(group), "beach")
                .as_deref(),
            Some("sunset")
        );
    }
}

#[test]
fn a_group_is_never_left_with_nobody_to_carry_it_on() {
    let (mut sim, group) = Sim::group(2, 2);
    let dead = [
        vec![(sim.id(PHONE), Role::witness(1))],
        vec![(sim.id(PHONE), Role::contributor(0))],
        vec![],
    ];

    for roles in dead {
        let asked = block_on(sim.devices[PHONE].reconfigure(group, roles));
        assert!(asked.is_err());
    }
}
