//! Taking the secret away: whoever stops being named stops reading.

use super::*;
use crate::chain::Gossip;
use crate::db::group::Handed;
use crate::db::message::{Message, Summary};
use crate::encoding;

const LATER: &str = "we moved it to sunday";

/// The roles of [`sealed`], without some.
fn without(sim: &Sim, gone: &[usize]) -> Vec<(usize, Role)> {
    let mut roles = roles(sim).to_vec();
    roles.retain(|(device, _)| !gone.contains(device));

    roles
}

/// A sealed group with one thing written, and then the friend, who
/// read it, removed.
fn removed() -> (Sim, GroupId) {
    let (mut sim, group) = sealed();
    sim.write_to(PHONE, group, set(notes(group), "plan", NEEDLE));
    sim.settle();
    let roles = without(&sim, &[FRIEND]);
    sim.assign(PHONE, group, roles);
    sim.settle();

    (sim, group)
}

#[test]
fn whoever_is_removed_reads_nothing_written_since() {
    let (mut sim, group) = removed();

    sim.write_to(PHONE, group, set(notes(group), "plan", LATER));
    let told = sent(&mut sim, LATER);
    sim.settle();

    assert_eq!(told, BTreeSet::new());
    for device in [PHONE, LAPTOP] {
        assert!(sim.devices[device].reads(group));
        let plan = read_note(&sim, device, group, "plan");
        assert_eq!(plan.as_deref(), Some(LATER), "device {device}");
    }

    // It keeps what it had. Removal can't unshare.
    assert!(!sim.devices[FRIEND].reads(group));
    assert!(!stores(&sim, FRIEND, group, LATER));
    let plan = read_note(&sim, FRIEND, group, "plan");
    assert_eq!(plan.as_deref(), Some(NEEDLE));
}

#[test]
fn a_removed_contributor_reads_nothing_written_since() {
    let (mut sim, group) = sealed();
    let roles = without(&sim, &[LAPTOP]);
    sim.assign(PHONE, group, roles);
    sim.settle();

    sim.write_to(PHONE, group, set(notes(group), "plan", LATER));
    sim.settle();

    assert_eq!(
        read_note(&sim, FRIEND, group, "plan").as_deref(),
        Some(LATER)
    );
    assert!(!sim.devices[LAPTOP].reads(group));
    assert_eq!(read_note(&sim, LAPTOP, group, "plan"), None);
    assert!(!stores(&sim, LAPTOP, group, LATER));
}

/// The guest is given the secret in force and nothing else. Each
/// secret opens the way to the one before, so that's enough to read
/// what was written before anyone was removed.
#[test]
fn one_secret_reads_the_groups_whole_past() {
    let (mut sim, group) = removed();
    sim.write_to(LAPTOP, group, set(notes(group), "cake", "lemon"));
    sim.settle();
    let mut roles = without(&sim, &[FRIEND, NAS]);
    roles.push((GUEST, Role::reader(key(&sim, GUEST))));
    sim.assign(PHONE, group, roles);
    sim.settle();

    for device in [GUEST, LAPTOP] {
        sim.restart(device);
    }
    sim.connect_all();
    sim.write_to(PHONE, group, set(notes(group), "when", LATER));
    sim.settle();

    for (key, value) in [("plan", NEEDLE), ("cake", "lemon"), ("when", LATER)] {
        for device in [GUEST, LAPTOP] {
            let note = read_note(&sim, device, group, key);
            assert_eq!(note.as_deref(), Some(value), "device {device}, {key}");
        }
    }
}

/// A contributor who hasn't heard of a removal still writes under the
/// secret it has, and whoever was removed can still read that.
#[test]
fn what_is_written_under_an_old_secret_is_read_with_it() {
    let (mut sim, group) = sealed();
    sim.isolate(LAPTOP);
    let roles = without(&sim, &[FRIEND]);
    sim.assign(PHONE, group, roles);
    sim.settle();

    sim.write_to(LAPTOP, group, set(notes(group), "cake", "lemon"));
    sim.write_to(PHONE, group, set(notes(group), "plan", LATER));
    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP] {
        let cake = read_note(&sim, device, group, "cake");
        assert_eq!(cake.as_deref(), Some("lemon"), "device {device}");
        let plan = read_note(&sim, device, group, "plan");
        assert_eq!(plan.as_deref(), Some(LATER), "device {device}");
    }
    assert!(!stores(&sim, FRIEND, group, LATER));
}

fn leaving(gone: &[usize], tamper: impl FnOnce(&mut Option<Sealing>)) -> [bool; 2] {
    judged_of(without(&Sim::new(5), gone), tamper)
}

#[test]
fn a_checkpoint_that_removes_a_reader_brings_in_a_new_secret() {
    let sim = Sim::new(5);
    let peer = |device: usize| sim.devices[device].identity.peer();
    let (laptop, friend) = (peer(LAPTOP), peer(FRIEND));
    let mut old = None;
    leaving(&[], |said| old = Some(sealing(said).commitment));
    let old = old.expect("a sealed group");
    let stale = |said: &mut Option<Sealing>| {
        let sealing = sealing(said);
        sealing.epoch = 0;
        sealing.commitment = old;
        sealing.previous = None;
    };
    let to_the_removed = |said: &mut Option<Sealing>| {
        let sealing = sealing(said);
        let copy = sealing.grants[&laptop].clone();
        sealing.grants.insert(friend, copy);
    };

    assert_eq!(leaving(&[FRIEND], |_| {}), [true, true]);
    assert_eq!(leaving(&[FRIEND], stale), [false, false]);
    assert_eq!(
        leaving(&[FRIEND], |said| sealing(said).epoch = 0),
        [false, false]
    );
    assert_eq!(
        leaving(&[FRIEND], |said| sealing(said).previous = None),
        [false, false]
    );
    assert_eq!(
        leaving(&[FRIEND], |said| drop(sealing(said).grants.remove(&laptop))),
        [false, false]
    );
    assert_eq!(leaving(&[FRIEND], to_the_removed), [false, false]);
}

/// What only someone with the new secret can tell.
#[test]
fn a_new_secret_has_to_open_the_way_to_the_one_before() {
    let dead_end = |said: &mut Option<Sealing>| sealing(said).previous = Some(vec![7; 80]);
    let another = |said: &mut Option<Sealing>| {
        sealing(said).commitment = another_secret().commitment();
    };

    assert_eq!(leaving(&[FRIEND], dead_end), [false, true]);
    assert_eq!(leaving(&[FRIEND], another), [false, true]);
}

/// An attempt that would have kept someone hands them a secret. If
/// another attempt wins and removes them, that isn't the one it uses.
#[test]
fn a_secret_is_for_one_set_of_readers() {
    let commitment = |gone: &[usize]| {
        let mut found = None;
        leaving(gone, |said| found = Some(sealing(said).commitment));

        found.expect("a sealed group")
    };

    assert_ne!(commitment(&[FRIEND]), commitment(&[FRIEND, NAS, LAPTOP]));
    assert_eq!(commitment(&[FRIEND]), commitment(&[FRIEND]));
}

#[test]
fn a_checkpoint_that_removes_nobody_keeps_the_secret() {
    let wrapped = |said: &mut Option<Sealing>| sealing(said).previous = Some(vec![7; 80]);

    assert_eq!(leaving(&[], |_| {}), [true, true]);
    assert_eq!(leaving(&[], wrapped), [false, false]);
}

/// A secret handed over is checked against what the chain commits
/// to, whoever hands it.
#[test]
fn a_secret_handed_over_has_to_be_the_one_the_chain_names() {
    let (mut sim, group) = sealed();
    sim.write_to(PHONE, group, set(notes(group), "plan", NEEDLE));
    sim.settle();
    let guest = sim.devices[GUEST].identity.peer();
    let about = crate::db::group::sealed_about(group, 0);
    let grant =
        |sim: &Sim| another_secret().grant(&sim.devices[NAS].identity, &key(sim, GUEST), &about);
    let wrong = grant(&sim);
    sim.isolate(GUEST);
    put_forward(&mut sim, group, |said| {
        sealing(said).grants.insert(guest, wrong);
    });

    // The NAS has the chain and no secret to give.
    sim.connect(GUEST, NAS);
    sim.settle();
    assert!(
        sim.devices[GUEST].groups[&group]
            .tip()
            .electorate
            .is_reader(guest)
    );
    assert!(!sim.devices[GUEST].reads(group));

    let message = Message {
        group,
        summary: Summary::default(),
        chain: Gossip::default(),
        entries: Vec::new(),
        links: Vec::new(),
        restored: Vec::new(),
        proofs: Vec::new(),
        snapshot: None,
        requests: Vec::new(),
        secret: Some(Handed {
            epoch: 0,
            grant: grant(&sim),
        }),
    };
    let from = sim.id(NAS);
    let received = block_on(sim.devices[GUEST].receive(from, &encoding::encode(&message)));
    sim.absorb(GUEST, received.expect("received"));
    sim.settle();
    assert!(!sim.devices[GUEST].reads(group));

    sim.connect(GUEST, FRIEND);
    sim.settle();
    assert!(sim.devices[GUEST].reads(group));
    assert_eq!(
        read_note(&sim, GUEST, group, "plan").as_deref(),
        Some(NEEDLE)
    );
}
