//! A group that has lost its quorum for good, and the one founded to
//! carry on from it.

use super::*;

/// Three devices decide, and two of them are gone. What's left is
/// what they committed together, and something the phone wrote since.
fn stranded() -> (Sim, GroupId) {
    let (mut sim, group) = Sim::group(4, 3);
    sim.write(PHONE, set(notes(group), "todo", "milk"));
    sim.write(LAPTOP, set(notes(group), "done", "bread"));
    sim.settle();
    sim.isolate(LAPTOP);
    sim.isolate(NAS);
    sim.write(PHONE, set(notes(group), "later", "eggs"));
    sim.settle();
    assert_eq!(sim.committed(PHONE, group, notes(group), "later"), None);

    (sim, group)
}

/// Founds a group to carry on from `old`, and brings its state over.
fn succeed(sim: &mut Sim, device: usize, old: GroupId) -> GroupId {
    sim.wake(device);
    let founded = block_on(sim.devices[device].succeed(old, [9; 32]));
    let (new, outputs) = founded.expect("founded");
    sim.absorb(device, outputs);
    sim.run();

    new
}

fn join(sim: &mut Sim, device: usize, group: GroupId, inviter: usize) {
    let invitation = sim.devices[inviter].invite(group).expect("held");
    sim.wake(device);
    let (_, outputs) = block_on(sim.devices[device].join(&invitation)).expect("joined");
    sim.absorb(device, outputs);
}

#[test]
fn a_successor_starts_with_what_the_old_group_committed() {
    let (mut sim, old) = stranded();
    let new = succeed(&mut sim, PHONE, old);
    let committed = |sim: &Sim, group, key| sim.committed(PHONE, group, notes(old), key);

    assert_ne!(new, old);
    assert_eq!(sim.devices[PHONE].succeeds(new), Some(old));
    assert_eq!(sim.devices[PHONE].succeeds(old), None);
    assert_eq!(committed(&sim, new, "todo").as_deref(), Some("milk"));
    assert_eq!(committed(&sim, new, "done").as_deref(), Some("bread"));

    // What wasn't committed isn't carried over, and is still where it
    // was written.
    assert_eq!(sim.shown(PHONE, new, notes(old), "later"), None);
    assert_eq!(
        sim.shown(PHONE, old, notes(old), "later").as_deref(),
        Some("eggs")
    );

    // The phone decides alone now.
    sim.write_to(PHONE, new, set(notes(old), "later", "eggs"));
    sim.settle();
    assert_eq!(committed(&sim, new, "later").as_deref(), Some("eggs"));
    assert_eq!(committed(&sim, old, "later"), None);
}

/// A survivor is told of the successor, joins it, and is sent what it
/// starts from. Once it's given a say, the two carry on.
#[test]
fn a_survivor_joins_the_successor_and_is_sent_its_state() {
    let (mut sim, old) = stranded();
    let new = succeed(&mut sim, PHONE, old);
    sim.write_to(PHONE, new, set(notes(old), "later", "eggs"));
    join(&mut sim, FRIEND, new, PHONE);
    sim.settle();

    let read = |sim: &Sim, key| sim.committed(FRIEND, new, notes(old), key);
    assert_eq!(read(&sim, "todo").as_deref(), Some("milk"));
    assert_eq!(read(&sim, "later").as_deref(), Some("eggs"));
    assert_eq!(sim.devices[FRIEND].succeeds(new), Some(old));

    sim.reconfigure(PHONE, new, [PHONE, FRIEND]);
    sim.settle();
    sim.write_to(FRIEND, new, set(notes(old), "todo", "oat milk"));
    sim.settle();
    let todo = sim.committed(PHONE, new, notes(old), "todo");
    assert_eq!(todo.as_deref(), Some("oat milk"));
}

/// A device that joins before the founder has finished, or has moved
/// on, still ends up with the state.
#[test]
fn a_survivor_that_joins_at_the_founding_waits_for_its_state() {
    let (mut sim, old) = stranded();
    sim.wake(PHONE);
    let (new, outputs) = block_on(sim.devices[PHONE].succeed(old, [9; 32])).expect("founded");
    sim.absorb(PHONE, outputs);
    join(&mut sim, FRIEND, new, PHONE);
    assert_eq!(sim.committed(FRIEND, new, notes(old), "todo"), None);

    // Until it has, it can't say what another would start from.
    let founded = block_on(sim.devices[FRIEND].succeed(new, [5; 32]));
    assert!(matches!(founded, Err(Error::Unreadable)));
    sim.settle();

    let todo = sim.committed(FRIEND, new, notes(old), "todo");
    assert_eq!(todo.as_deref(), Some("milk"));
}

/// More than comes over in one step, and a restart partway.
#[test]
fn a_large_state_is_brought_over_a_part_at_a_time() {
    let (mut sim, old) = Sim::group(1, 1);
    for batch in 0..4 {
        let mut transaction = Transaction::new();
        for key in 0..100 {
            transaction = transaction.set(notes(old), format!("{batch}/{key:03}"), "value");
        }
        sim.write(PHONE, transaction);
    }
    sim.settle();

    sim.wake(PHONE);
    let (new, outputs) = block_on(sim.devices[PHONE].succeed(old, [9; 32])).expect("founded");
    assert!(outputs.unfinished);
    sim.absorb(PHONE, outputs);
    assert!(sim.resume());
    assert_eq!(sim.committed(PHONE, new, notes(old), "3/099"), None);
    sim.restart(PHONE);
    sim.run();

    for key in ["0/000", "1/050", "3/099"] {
        let read = sim.committed(PHONE, new, notes(old), key);
        assert_eq!(read.as_deref(), Some("value"), "{key}");
    }
    sim.write_to(PHONE, new, set(notes(old), "0/000", "changed"));
    sim.settle();
    let read = sim.committed(PHONE, new, notes(old), "0/000");
    assert_eq!(read.as_deref(), Some("changed"));
}

/// The founding names one checkpoint, and bringing its state over
/// takes a while. The old group commits nothing more on this device
/// until that's done, through a restart, and then carries on.
#[test]
fn the_old_group_waits_while_its_state_is_brought_over() {
    let (mut sim, old) = Sim::group(1, 1);
    let mut transaction = Transaction::new();
    for key in 0..300 {
        transaction = transaction.set(notes(old), format!("{key:03}"), "value");
    }
    sim.write(PHONE, transaction);
    sim.settle();

    sim.wake(PHONE);
    let (new, outputs) = block_on(sim.devices[PHONE].succeed(old, [9; 32])).expect("founded");
    sim.absorb(PHONE, outputs);
    sim.write_to(PHONE, old, set(notes(old), "000", "changed"));
    for _ in 0..4 {
        sim.tick(PHONE);
    }
    sim.restart(PHONE);
    for _ in 0..4 {
        sim.tick(PHONE);
    }
    let waiting = sim.committed(PHONE, old, notes(old), "000");
    assert_eq!(waiting.as_deref(), Some("value"));
    assert_eq!(sim.committed(PHONE, new, notes(old), "299"), None);
    sim.settle();

    for key in ["000", "001", "299"] {
        let brought = sim.committed(PHONE, new, notes(old), key);
        assert_eq!(brought.as_deref(), Some("value"), "key {key}");
    }
    let changed = sim.committed(PHONE, old, notes(old), "000");
    assert_eq!(changed.as_deref(), Some("changed"));
    assert!(!sim.devices[PHONE].groups[&new].is_busy());
}

/// A model that was dropped stays dropped.
#[test]
fn a_dropped_model_is_dropped_in_the_successor() {
    let (mut sim, old) = Sim::group(1, 1);
    let scratch = ModelId::kv(old, [8; 32]);
    sim.write(PHONE, set(notes(old), "todo", "milk"));
    sim.write(PHONE, set(scratch, "draft", "one"));
    sim.settle();
    sim.write(PHONE, Transaction::new().drop_model(scratch));
    sim.settle();

    let new = succeed(&mut sim, PHONE, old);
    sim.write_to(PHONE, new, set(scratch, "draft", "two"));
    sim.write_to(PHONE, new, set(notes(old), "todo", "tea"));
    sim.settle();

    assert_eq!(sim.committed(PHONE, new, scratch, "draft"), None);
    let todo = sim.committed(PHONE, new, notes(old), "todo");
    assert_eq!(todo.as_deref(), Some("tea"));
}

/// Only a device that holds all of the old group's state, and isn't
/// partway to it, can say what a successor starts from.
#[test]
fn a_device_without_the_whole_state_cant_found_a_successor() {
    let (mut sim, old) = stranded();
    let clocks = Some(BTreeSet::new());
    sim.wake(FRIEND);
    let narrowed = block_on(sim.devices[FRIEND].want(old, clocks)).expect("narrowed");
    sim.absorb(FRIEND, narrowed);

    let founded = block_on(sim.devices[FRIEND].succeed(old, [9; 32]));
    assert!(matches!(founded, Err(Error::Unreadable)));
    let unknown = block_on(sim.devices[FRIEND].succeed(GroupId::from_bytes([3; 32]), [9; 32]));
    assert!(matches!(unknown, Err(Error::UnknownGroup)));
}

/// What's written to a successor before its state is in waits for it,
/// and goes on top.
#[test]
fn a_write_made_before_the_state_is_in_lands_on_top_of_it() {
    let (mut sim, old) = Sim::group(1, 1);
    let mut transaction = Transaction::new();
    for key in 0..300 {
        transaction = transaction.set(notes(old), format!("{key:03}"), "value");
    }
    sim.write(PHONE, transaction);
    sim.settle();

    sim.wake(PHONE);
    let (new, outputs) = block_on(sim.devices[PHONE].succeed(old, [9; 32])).expect("founded");
    sim.absorb(PHONE, outputs);
    sim.write_to(PHONE, new, set(notes(old), "000", "changed"));
    sim.tick(PHONE);
    assert_eq!(sim.committed(PHONE, new, notes(old), "000"), None);
    sim.settle();

    let read = |key| sim.committed(PHONE, new, notes(old), key);
    assert_eq!(read("000").as_deref(), Some("changed"));
    assert_eq!(read("299").as_deref(), Some("value"));
}

/// The successor of a sealed group is sealed, under another secret.
/// Whoever is given a role reads what it started with, and nobody
/// else does.
#[test]
fn a_sealed_group_is_succeeded_by_a_sealed_one() {
    let mut sim = Sim::new(3);
    let (old, outputs) = block_on(sim.devices[PHONE].found_sealed([7; 32])).expect("founded");
    sim.absorb(PHONE, outputs);
    let mut transaction = Transaction::new();
    for key in 0..300 {
        transaction = transaction.set(notes(old), format!("{key:03}"), "value");
    }
    sim.write_to(PHONE, old, transaction);
    sim.settle();

    let new = succeed(&mut sim, PHONE, old);
    assert!(sim.devices[PHONE].groups[&new].is_sealed());
    let read = |sim: &Sim, device, key| sim.committed(device, new, notes(old), key);
    assert_eq!(read(&sim, PHONE, "000").as_deref(), Some("value"));
    assert_eq!(read(&sim, PHONE, "299").as_deref(), Some("value"));

    sim.connect_all();
    join(&mut sim, LAPTOP, new, PHONE);
    join(&mut sim, NAS, new, PHONE);
    let key = |sim: &Sim, device: usize| sim.devices[device].sealing_key();
    let roles = [
        (PHONE, Role::contributor(1).sealed_to(key(&sim, PHONE))),
        (LAPTOP, Role::reader(key(&sim, LAPTOP))),
    ];
    sim.assign(PHONE, new, roles);
    sim.write_to(PHONE, new, set(notes(old), "000", "changed"));
    sim.settle();

    assert_eq!(read(&sim, LAPTOP, "000").as_deref(), Some("changed"));
    assert_eq!(read(&sim, LAPTOP, "299").as_deref(), Some("value"));
    assert!(!sim.devices[NAS].reads(new));
    assert_eq!(read(&sim, NAS, "299"), None);
}

/// A device that can't read a group can't say what its successor
/// starts from.
#[test]
fn a_device_that_cant_read_a_group_cant_found_its_successor() {
    let mut sim = Sim::new(2);
    let (old, outputs) = block_on(sim.devices[PHONE].found_sealed([7; 32])).expect("founded");
    sim.absorb(PHONE, outputs);
    sim.write_to(PHONE, old, set(notes(old), "todo", "milk"));
    sim.connect_all();
    join(&mut sim, LAPTOP, old, PHONE);
    sim.settle();

    let founded = block_on(sim.devices[LAPTOP].succeed(old, [9; 32]));
    assert!(matches!(founded, Err(Error::Unreadable)));
}
