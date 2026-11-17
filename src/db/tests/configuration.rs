use super::*;
use crate::db::Configuration;

/// The phone, the laptop and the NAS, with the NAS counting double.
fn heavier_nas(sim: &Sim) -> Vec<(VerifyingKey, Role)> {
    vec![
        (sim.id(PHONE), Role::contributor(1)),
        (sim.id(LAPTOP), Role::contributor(1)),
        (sim.id(NAS), Role::contributor(2)),
    ]
}

/// A request is made of the group as it was. A device that learns the
/// group changed from a snapshot, and never commits the checkpoint
/// that changed it, lets its request go all the same.
#[test]
fn a_request_is_spent_on_a_device_that_caught_up_from_a_snapshot() {
    let (mut sim, group) = Sim::group(4, 3);
    sim.reconfigured[PHONE].clear();
    sim.isolate(PHONE);
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP, NAS, FRIEND]);

    let roles = heavier_nas(&sim);
    let outputs = block_on(sim.devices[LAPTOP].reconfigure(group, roles));
    sim.absorb(LAPTOP, outputs.expect("asked"));
    sim.settle();
    sim.write_to(LAPTOP, group, set(notes(group), "todo", "milk"));
    sim.settle();
    for device in [LAPTOP, NAS, FRIEND] {
        sim.compact(device, group);
    }

    sim.connect_all();
    sim.settle();
    sim.settle();

    assert_eq!(read_committed(&sim, PHONE, "todo").as_deref(), Some("milk"));
    let everything = Changed::Everything { group };
    assert!(sim.changes[PHONE].contains(&everything), "from a snapshot");
    assert_eq!(sim.reconfigured[PHONE], [(group, Reconfigured::Spent)]);
    assert_eq!(configuration(&sim, PHONE, group).asked, None);
    let written = block_on(sim.devices[FRIEND].write(group, set(notes(group), "todo", "eggs")));
    assert!(matches!(written, Err(Error::NotAContributor)));
}

fn configuration(sim: &Sim, device: usize, group: GroupId) -> Configuration {
    sim.devices[device].configuration(group).expect("held")
}

fn ask(sim: &mut Sim, device: usize, group: GroupId, roles: Vec<(VerifyingKey, Role)>) {
    let outputs = block_on(sim.devices[device].reconfigure(group, roles));
    sim.absorb(device, outputs.expect("asked"));
}

/// Whether the device was told, since the test last looked, that the
/// group is made up differently.
fn heard_of_members(sim: &mut Sim, device: usize, group: GroupId) -> bool {
    let changes = std::mem::take(&mut sim.changes[device]);

    changes.contains(&Changed::Members { group })
}

#[test]
fn a_device_reads_who_is_in_a_group_and_what_each_may_do() {
    let (mut sim, group) = Sim::group(3, 2);
    let social = ClockId::new(group, [9; 32]);
    let roles = vec![
        (sim.id(PHONE), Role::contributor(2)),
        (sim.id(LAPTOP), Role::contributor(0).only([social])),
        (sim.id(NAS), Role::witness(1)),
    ];
    ask(&mut sim, PHONE, group, roles.clone());

    let before = configuration(&sim, PHONE, group);
    let asked = before.asked.clone().expect("asked");
    assert_eq!(asked.len(), 3);
    assert!(roles.iter().all(|role| asked.contains(role)));
    assert_eq!(before.roles.len(), 2);

    sim.settle();

    // Every device that holds the group says the same, with a role in
    // it or not.
    for device in [PHONE, LAPTOP, NAS] {
        let held = configuration(&sim, device, group);
        assert!(held.height > before.height);
        assert!(!held.sealed);
        assert_eq!(held.asked, None);
        assert_eq!(held.roles.len(), 3);
        for (peer, role) in &roles {
            assert_eq!(held.role(peer), Some(role));
        }
    }

    let held = configuration(&sim, NAS, group);
    let role = |device: usize| held.role(&sim.id(device)).expect("a member");
    assert_eq!(role(PHONE).weight(), 2);
    assert!(role(PHONE).is_contributor() && role(PHONE).writes(social));
    assert_eq!(role(PHONE).clocks(), None);
    assert_eq!(role(LAPTOP).weight(), 0);
    assert!(role(LAPTOP).writes(social) && !role(LAPTOP).writes(group.into()));
    assert!(role(NAS).is_witness() && !role(NAS).writes(social));
    assert!(
        sim.devices[PHONE]
            .configuration(GroupId::from_bytes([3; 32]))
            .is_none()
    );
}

#[test]
fn a_sealed_groups_configuration_names_each_readers_key() {
    let mut sim = Sim::new(2);
    let (group, outputs) = block_on(sim.devices[PHONE].found_sealed([7; 32])).expect("founded");
    sim.absorb(PHONE, outputs);
    let invitation = sim.devices[PHONE].invite(group).expect("held");
    let (_, outputs) = block_on(sim.devices[LAPTOP].join(&invitation)).expect("joined");
    sim.absorb(LAPTOP, outputs);

    // A device that can't read the group reads who's in it.
    let held = configuration(&sim, LAPTOP, group);
    let key = sim.devices[PHONE].sealing_key();

    assert!(held.sealed);
    assert_eq!(held.roles.len(), 1);
    assert_eq!(
        held.role(&sim.id(PHONE)).and_then(Role::sealing_key),
        Some(&key)
    );
    assert_eq!(held.role(&sim.id(LAPTOP)), None);
}

#[test]
fn every_device_hears_when_the_group_is_made_up_differently() {
    let (mut sim, group) = Sim::group(3, 2);
    for device in [PHONE, LAPTOP, NAS] {
        sim.changes[device].clear();
    }

    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();
    for device in [PHONE, LAPTOP, NAS] {
        assert!(!heard_of_members(&mut sim, device, group));
    }

    sim.reconfigure(PHONE, group, [PHONE, LAPTOP, NAS]);
    sim.settle();
    for device in [PHONE, LAPTOP, NAS] {
        assert!(heard_of_members(&mut sim, device, group));
    }

    // The NAS was told when it could write, and now can.
    sim.write_to(NAS, group, set(notes(group), "todo", "eggs"));
    assert_eq!(
        sim.reconfigured[PHONE].last(),
        Some(&(group, Reconfigured::Carried))
    );
    assert_eq!(sim.reconfigured[LAPTOP], []);
}

/// Two owners, apart, each admit a guest. A request names everyone, so
/// only one of the two can be carried: the other was made of a group
/// that's since changed. Its author is told, reads who's in the group
/// now, and asks again.
#[test]
fn a_request_made_of_a_group_that_changed_is_spent_and_its_author_told() {
    let (mut sim, group) = Sim::group(4, 2);
    let owners = [PHONE, LAPTOP].map(|it| (sim.id(it), Role::contributor(1)));
    let guest = |sim: &Sim, device: usize| (sim.id(device), Role::contributor(0));
    sim.reconfigured[PHONE].clear();
    sim.isolate(PHONE);
    sim.isolate(LAPTOP);
    let with_nas = owners.iter().cloned().chain([guest(&sim, NAS)]).collect();
    let with_friend = owners
        .iter()
        .cloned()
        .chain([guest(&sim, FRIEND)])
        .collect();
    ask(&mut sim, PHONE, group, with_nas);
    ask(&mut sim, LAPTOP, group, with_friend);

    sim.connect_all();
    sim.settle();
    sim.settle();

    let told = |sim: &Sim, device: usize| sim.reconfigured[device].clone();
    let (carried, spent) = match (&told(&sim, PHONE)[..], &told(&sim, LAPTOP)[..]) {
        ([(_, Reconfigured::Carried)], [(_, Reconfigured::Spent)]) => (PHONE, LAPTOP),
        ([(_, Reconfigured::Spent)], [(_, Reconfigured::Carried)]) => (LAPTOP, PHONE),
        told => panic!("one carried and one spent, not {told:?}"),
    };
    let left_out = if carried == PHONE { FRIEND } else { NAS };
    let held = configuration(&sim, spent, group);
    assert_eq!(held.asked, None);
    assert_eq!(held.roles.len(), 3);
    assert_eq!(held.role(&sim.id(left_out)), None);

    // What's in force, and the guest it left out.
    let mut roles = held.roles;
    roles.push(guest(&sim, left_out));
    ask(&mut sim, spent, group, roles);
    sim.settle();
    sim.settle();

    assert_eq!(
        told(&sim, spent).last(),
        Some(&(group, Reconfigured::Carried))
    );
    for device in [NAS, FRIEND] {
        sim.write_to(device, group, set(notes(group), "todo", "milk"));
    }
}

/// What a request came to is told once. A device that stopped asks
/// what's in force instead.
#[test]
fn a_request_that_still_stands_is_read_after_a_restart() {
    let (mut sim, group) = Sim::group(3, 2);
    sim.isolate(PHONE);
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP, NAS]);
    sim.restart(PHONE);

    assert_eq!(
        configuration(&sim, PHONE, group).asked.map(|it| it.len()),
        Some(3)
    );

    sim.connect_all();
    sim.settle();

    assert_eq!(configuration(&sim, PHONE, group).asked, None);
    assert_eq!(configuration(&sim, PHONE, group).roles.len(), 3);
}
