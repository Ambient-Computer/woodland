//! Letting go of a group, and everything kept for it.

use super::*;
use crate::store::{Order, Store, View};

/// A household that has written something, and all heard of it.
fn settled() -> (Sim, GroupId) {
    let (mut sim, group) = Sim::group(4, 3);
    sim.write_to(PHONE, group, set(notes(group), "kept", "yes"));
    sim.settle();

    (sim, group)
}

fn forget(sim: &mut Sim, device: usize, group: GroupId) -> Result<(), Error> {
    let outputs = block_on(sim.devices[device].forget(group))?;
    sim.absorb(device, outputs);

    Ok(())
}

/// How many keys the device's store holds for any group at all.
fn kept(sim: &Sim, device: usize) -> usize {
    let view = sim.devices[device].store.view();
    let groups = block_on(view.scan(&[1], Some(&[3]), Order::Ascending, 10_000));

    groups.expect("scanned").len()
}

#[test]
fn a_device_with_no_role_can_forget_a_group() {
    let (mut sim, group) = settled();
    assert!(kept(&sim, FRIEND) > 0);

    forget(&mut sim, FRIEND, group).expect("forgotten");

    assert_eq!(sim.devices[FRIEND].groups().count(), 0);
    assert_eq!(kept(&sim, FRIEND), 0, "nothing is left behind");
    let read = block_on(sim.devices[FRIEND].read(&Query::key(group, notes(group), b"kept")));
    assert!(matches!(read, Err(Error::UnknownGroup)) || read.is_ok_and(|it| it.found().is_empty()));

    // The rest carry on, and what they send it falls on deaf ears.
    sim.write_to(LAPTOP, group, set(notes(group), "later", "yes"));
    sim.settle();
    assert_eq!(
        sim.committed(NAS, group, notes(group), "later").as_deref(),
        Some("yes")
    );
    assert_eq!(kept(&sim, FRIEND), 0);
}

#[test]
fn it_stays_forgotten() {
    let (mut sim, group) = settled();
    forget(&mut sim, FRIEND, group).expect("forgotten");

    sim.restart(FRIEND);

    assert_eq!(sim.devices[FRIEND].groups().count(), 0);
}

#[test]
fn a_member_cant_forget_its_group() {
    let (mut sim, group) = settled();

    let refused = forget(&mut sim, NAS, group);

    assert!(matches!(refused, Err(Error::StillAMember)));
    assert_eq!(
        sim.shown(NAS, group, notes(group), "kept").as_deref(),
        Some("yes")
    );
}

#[test]
fn a_group_nobody_here_holds_cant_be_forgotten() {
    let (mut sim, group) = settled();
    forget(&mut sim, FRIEND, group).expect("forgotten");

    assert!(matches!(
        forget(&mut sim, FRIEND, group),
        Err(Error::UnknownGroup)
    ));
}

fn abandon(sim: &mut Sim, device: usize, group: GroupId) -> Result<(), Error> {
    let outputs = block_on(sim.devices[device].abandon(group))?;
    sim.absorb(device, outputs);

    Ok(())
}

/// The last device a group has can't be removed by anyone. It lets
/// the group go for good.
#[test]
fn the_last_member_abandons_its_group_and_keeps_none_of_it() {
    let (mut sim, group) = Sim::group(1, 1);
    sim.write_to(PHONE, group, set(notes(group), "kept", "yes"));
    sim.settle();
    assert!(matches!(
        forget(&mut sim, PHONE, group),
        Err(Error::StillAMember)
    ));

    abandon(&mut sim, PHONE, group).expect("abandoned");

    assert_eq!(sim.devices[PHONE].groups().count(), 0);
    assert_eq!(kept(&sim, PHONE), 0, "nothing is left behind");
    let everything = Changed::Everything { group };
    assert!(sim.changes[PHONE].contains(&everything));
}

/// It signed things there and forgot them. Founded again from the
/// same seed it would be the same group, and joined again it would
/// be the same member: neither happens, before a restart or after.
#[test]
fn an_abandoned_group_is_never_held_again() {
    let (mut sim, group) = settled();
    let invitation = sim.devices[PHONE].invite(group).expect("held");
    abandon(&mut sim, NAS, group).expect("abandoned");

    let joined = block_on(sim.devices[NAS].join(&invitation));
    assert!(matches!(joined, Err(Error::Abandoned)));
    sim.restart(NAS);
    let joined = block_on(sim.devices[NAS].join(&invitation));
    assert!(matches!(joined, Err(Error::Abandoned)));
    assert_eq!(sim.devices[NAS].groups().count(), 0);

    let (mut sim, group) = Sim::group(1, 1);
    abandon(&mut sim, PHONE, group).expect("abandoned");
    let founded = block_on(sim.devices[PHONE].found([7; 32]));
    assert!(matches!(founded, Err(Error::Abandoned)));
    let (other, _) = block_on(sim.devices[PHONE].found([8; 32])).expect("another group");
    assert_ne!(other, group);
}

/// To the others it's a device that was lost: they wait on its
/// weight until they remove it, which they still can.
#[test]
fn the_rest_carry_on_once_they_remove_whoever_abandoned_them() {
    let (mut sim, group) = settled();
    abandon(&mut sim, NAS, group).expect("abandoned");

    sim.write_to(LAPTOP, group, set(notes(group), "later", "yes"));
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();

    let later = sim.committed(PHONE, group, notes(group), "later");
    assert_eq!(later.as_deref(), Some("yes"));
    assert!(!sim.devices[PHONE].groups[&group].has(sim.id(NAS).into()));
    // What they send it falls on deaf ears.
    assert_eq!(kept(&sim, NAS), 0);
}

#[test]
fn a_device_with_no_role_has_nothing_to_abandon() {
    let (mut sim, group) = settled();

    assert!(matches!(
        abandon(&mut sim, FRIEND, group),
        Err(Error::NotAMember)
    ));
    let stranger = GroupId::from_bytes([9; 32]);
    assert!(matches!(
        abandon(&mut sim, FRIEND, stranger),
        Err(Error::UnknownGroup)
    ));
    assert_eq!(sim.devices[FRIEND].groups().count(), 1);
}

/// A group that lost its quorum is carried on from, and then let go
/// of: but not while the group that carries on is still copying it.
#[test]
fn a_group_is_abandoned_once_its_successor_has_its_state() {
    let (mut sim, old) = settled();
    sim.isolate(LAPTOP);
    sim.isolate(NAS);
    let (new, outputs) = block_on(sim.devices[PHONE].succeed(old, [9; 32])).expect("founded");
    sim.absorb(PHONE, outputs);
    assert!(matches!(
        abandon(&mut sim, PHONE, old),
        Err(Error::Bequeathing)
    ));

    sim.run();
    abandon(&mut sim, PHONE, old).expect("abandoned");

    assert_eq!(sim.devices[PHONE].groups().collect::<Vec<_>>(), [new]);
    let kept = sim.committed(PHONE, new, notes(old), "kept");
    assert_eq!(kept.as_deref(), Some("yes"));
    sim.restart(PHONE);
    let kept = sim.committed(PHONE, new, notes(old), "kept");
    assert_eq!(kept.as_deref(), Some("yes"));
}

#[test]
fn a_removed_member_can_forget_and_be_invited_back() {
    let (mut sim, group) = settled();
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();

    forget(&mut sim, NAS, group).expect("forgotten");
    let invitation = sim.devices[PHONE].invite(group).expect("held");
    let (_, outputs) = block_on(sim.devices[NAS].join(&invitation)).expect("joined");
    sim.absorb(NAS, outputs);
    sim.settle();

    assert_eq!(
        sim.committed(NAS, group, notes(group), "kept").as_deref(),
        Some("yes")
    );
}
