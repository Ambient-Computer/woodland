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
    assert!(matches!(read, Err(Error::UnknownGroup)) || read.is_ok_and(|it| it.is_empty()));

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
