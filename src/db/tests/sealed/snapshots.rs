//! Committed state of a sealed group, on its way to devices that
//! read it and through devices that don't.

use super::*;
use crate::chain::Gossip;
use crate::db::message::{Message, Piece, SealedPiece, Summary};
use crate::encoding;

/// Enough notes that a snapshot comes in several pieces.
const MANY: usize = 300;

fn folded(sim: &Sim, device: usize, group: GroupId) -> u64 {
    sim.devices[device].groups[&group].folded()
}

/// A sealed group with a lot written, whose contributors have shed
/// all of its history. Nobody is connected.
fn shed() -> (Sim, GroupId) {
    let (mut sim, group) = sealed();
    for batch in 0..3 {
        let mut transaction = Transaction::new();
        for index in (0..MANY).filter(|index| index % 3 == batch) {
            transaction = transaction.set(notes(group), format!("note {index:03}"), NEEDLE);
        }
        sim.write_to(PHONE, group, transaction);
    }
    sim.settle();
    for device in [PHONE, LAPTOP] {
        sim.compact(device, group);
    }
    sim.disconnect_all();

    (sim, group)
}

fn reads_it_all(sim: &Sim, device: usize, group: GroupId) -> bool {
    let read = |index: usize| read_note(sim, device, group, &format!("note {index:03}"));

    (0..MANY).all(|index| read(index).as_deref() == Some(NEEDLE))
}

/// Once history is shed, what's left to send a newcomer is state. It
/// goes to anyone, and only those who read the group can read it.
#[test]
fn committed_state_travels_sealed() {
    let (mut sim, group) = shed();
    for device in [FRIEND, GUEST] {
        sim.rejoin(device, group, PHONE);
        sim.connect(device, PHONE);
    }
    let told = sent(&mut sim, NEEDLE);
    sim.settle();

    assert_eq!(told, BTreeSet::new());
    assert!(reads_it_all(&sim, FRIEND, group));
    assert!(!stores(&sim, GUEST, group, NEEDLE));
    assert_eq!(read_note(&sim, GUEST, group, "note 000"), None);

    // The guest is as far along as anyone, for all it can't read.
    assert_eq!(folded(&sim, GUEST, group), folded(&sim, PHONE, group));
}

/// The NAS can't read the state it's handed. The friend, who only
/// ever meets the NAS, gets it from there and can.
#[test]
fn a_device_that_cant_read_committed_state_passes_it_on() {
    let (mut sim, group) = shed();
    sim.rejoin(NAS, group, PHONE);
    sim.connect(NAS, PHONE);
    sim.settle();
    sim.restart(NAS);
    sim.rejoin(FRIEND, group, NAS);
    sim.connect(FRIEND, NAS);

    // Not what the notes say, and not what they're called either.
    let told = sent(&mut sim, "note 2");
    sim.settle();

    assert_eq!(told, BTreeSet::new());
    assert!(reads_it_all(&sim, FRIEND, group));
    assert!(!stores(&sim, NAS, group, NEEDLE));
    assert!(!stores(&sim, NAS, group, "note 2"));
    assert_eq!(folded(&sim, NAS, group), folded(&sim, PHONE, group));
}

#[test]
fn what_is_written_after_a_carried_snapshot_is_carried_too() {
    let (mut sim, group) = shed();
    sim.rejoin(NAS, group, PHONE);
    sim.connect(NAS, PHONE);
    sim.settle();
    sim.write_to(PHONE, group, set(notes(group), "plan", "sunday"));
    sim.settle();
    sim.disconnect_all();

    sim.rejoin(FRIEND, group, NAS);
    sim.connect(FRIEND, NAS);
    sim.settle();

    assert!(reads_it_all(&sim, FRIEND, group));
    assert_eq!(
        read_note(&sim, FRIEND, group, "plan").as_deref(),
        Some("sunday")
    );
}

/// It has no state to stand in for the history, so it keeps that.
#[test]
fn a_device_that_cant_read_sheds_nothing_on_its_own() {
    let (mut sim, group) = sealed();
    sim.write_to(PHONE, group, set(notes(group), "plan", NEEDLE));
    sim.settle();

    sim.compact(NAS, group);
    sim.compact(FRIEND, group);

    assert_eq!(sim.entries_kept(NAS, group), 1);
    assert_eq!(sim.entries_kept(FRIEND, group), 0);
}

fn offer(sim: &mut Sim, from: usize, to: usize, group: GroupId, piece: Piece) {
    let message = Message {
        group,
        summary: Summary::default(),
        chain: Gossip::default(),
        entries: Vec::new(),
        links: Vec::new(),
        restored: Vec::new(),
        proofs: Vec::new(),
        snapshot: Some(piece),
        requests: Vec::new(),
        secret: None,
    };
    let from = sim.id(from);
    let received = block_on(sim.devices[to].receive(from, &encoding::encode(&message)));
    sim.absorb(to, received.expect("received"));
}

/// A reader takes nothing for state that it can't open, and nothing
/// that wasn't sealed.
#[test]
fn a_reader_refuses_state_that_isnt_sealed_under_the_groups_secret() {
    let (mut sim, group) = shed();
    sim.rejoin(FRIEND, group, PHONE);
    assert!(sim.devices[FRIEND].reads(group));
    let height = folded(&sim, PHONE, group);
    assert!(folded(&sim, FRIEND, group) < height);

    let junk = Piece {
        height,
        start: None,
        values: Vec::new(),
        upto: None,
        end: None,
        sealed: Some(SealedPiece {
            body: vec![7; 200],
            last: true,
        }),
    };
    offer(&mut sim, NAS, FRIEND, group, junk);
    let clear = Piece {
        height,
        start: None,
        values: Vec::new(),
        upto: None,
        end: Some(BTreeSet::new()),
        sealed: None,
    };
    offer(&mut sim, NAS, FRIEND, group, clear);

    assert!(folded(&sim, FRIEND, group) < height);
    assert_eq!(read_note(&sim, FRIEND, group, "note 000"), None);
    sim.connect(FRIEND, PHONE);
    sim.settle();
    assert!(reads_it_all(&sim, FRIEND, group));
}

/// What it was carrying, it couldn't read. Given a key, it starts
/// over and is sent the state again, this time to keep.
#[test]
fn a_device_given_a_key_reads_the_state_it_was_carrying() {
    let (mut sim, group) = shed();
    sim.rejoin(GUEST, group, PHONE);
    sim.connect(GUEST, PHONE);
    sim.settle();
    assert_eq!(folded(&sim, GUEST, group), folded(&sim, PHONE, group));
    assert!(!sim.devices[GUEST].reads(group));

    sim.connect_all();
    let mut roles = roles(&sim).to_vec();
    roles.push((GUEST, Role::reader(key(&sim, GUEST))));
    sim.assign(PHONE, group, roles);
    sim.settle();

    assert!(reads_it_all(&sim, GUEST, group));
}
