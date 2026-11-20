//! The calls that stand in for what an application would write itself.

use super::tables::{age, create, insert, name, people, read_table};
use super::*;

fn delete(sim: &mut Sim, device: usize, table: ModelId, key: &str) -> bool {
    let group = sim.home;
    let deleted = block_on(sim.devices[device].delete_row(group, table, key.as_bytes()));
    let Some(outputs) = deleted.expect("read and written") else {
        return false;
    };
    sim.absorb(device, outputs);

    true
}

fn years(sim: &Sim, device: usize, table: ModelId, key: &str) -> Option<i64> {
    let read = read_table(sim, device, table)?;

    read.row(key.as_bytes())?.integer(age(table))
}

#[test]
fn a_row_is_deleted_by_its_key() {
    let mut sim = household();
    let table = people(&sim, 1);
    create(&mut sim, PHONE, table);
    insert(&mut sim, PHONE, table, "ada", 36);
    insert(&mut sim, PHONE, table, "bob", 41);
    sim.settle();

    assert!(delete(&mut sim, LAPTOP, table, "ada"));
    assert!(!delete(&mut sim, LAPTOP, table, "ada"), "it's gone already");
    assert!(!delete(&mut sim, LAPTOP, table, "nobody"));
    let never_made = people(&sim, 9);
    assert!(!delete(&mut sim, LAPTOP, never_made, "ada"), "no table");
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(years(&sim, device, table, "ada"), None, "device {device}");
        assert_eq!(years(&sim, device, table, "bob"), Some(41));
    }
}

/// Deleting by key is deleting the row that was there to read. A row
/// someone else has put under the key since is another row.
#[test]
fn deleting_by_key_leaves_a_later_row_under_the_same_key() {
    let mut sim = household();
    let table = people(&sim, 1);
    create(&mut sim, PHONE, table);
    insert(&mut sim, PHONE, table, "ada", 36);
    sim.settle();

    // The laptop deletes what it sees while it's away. The others
    // have deleted that row and put another in its place.
    sim.isolate(LAPTOP);
    assert!(delete(&mut sim, PHONE, table, "ada"));
    insert(&mut sim, PHONE, table, "ada", 37);
    sim.settle();
    assert!(delete(&mut sim, LAPTOP, table, "ada"));
    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(years(&sim, device, table, "ada"), Some(37), "{device}");
    }
}

#[test]
fn a_row_gives_what_it_holds_by_type() {
    let mut sim = household();
    let table = people(&sim, 1);
    create(&mut sim, PHONE, table);
    insert(&mut sim, PHONE, table, "ada", 36);
    let read = read_table(&sim, PHONE, table).expect("a table");
    let row = read.row(b"ada").expect("a row");

    assert_eq!(row.text(name(table)), Some("ada"));
    assert_eq!(row.integer(age(table)), Some(36));
    // Asked for as another type, or where nothing is held: nothing.
    assert_eq!(row.integer(name(table)), None);
    assert_eq!(row.text(age(table)), None);
    assert_eq!(row.boolean(age(table)), None);
    assert_eq!(row.real(age(table)), None);
    assert_eq!(row.bytes(age(table)), None);
    assert!(row.blob(age(table)).is_none());
    let never = crate::model::ColumnId::text(table, [8; 32]);
    assert_eq!(row.text(never), None);
}

/// Several steps are handled as one: every message, each change once.
#[test]
fn what_steps_ask_for_adds_up() {
    let group = GroupId::from_bytes([1; 32]);
    let key = |key: &str| Changed::Key {
        group,
        model: notes(group),
        key: key.as_bytes().to_vec(),
        committed: false,
    };
    let peer = fresh(LAPTOP).id();
    let mut first = Outputs {
        sends: vec![(peer, vec![1])],
        changed: vec![key("a"), key("b")],
        unfinished: true,
        blobs: vec![peer],
        ..Outputs::default()
    };
    let second = Outputs {
        sends: vec![(peer, vec![2])],
        changed: vec![key("b"), key("c")],
        reconfigured: vec![(group, Reconfigured::Spent)],
        ..Outputs::default()
    };

    first.absorb(second);

    assert_eq!(first.sends, [(peer, vec![1]), (peer, vec![2])]);
    assert_eq!(first.changed, [key("a"), key("b"), key("c")]);
    assert_eq!(first.reconfigured, [(group, Reconfigured::Spent)]);
    assert!(!first.unfinished);
    assert_eq!(first.blobs, []);
}

/// More than one step's work, seen through without the loop.
#[test]
fn settling_carries_on_until_nothing_is_unfinished() {
    let (mut sim, old) = Sim::group(1, 1);
    let mut transaction = Transaction::new();
    for key in 0..600 {
        transaction = transaction.set(notes(old), format!("{key:03}"), "value");
    }
    sim.write_to(PHONE, old, transaction);
    sim.settle();

    let founded = block_on(sim.devices[PHONE].succeed(old, [9; 32]));
    let (new, mut outputs) = founded.expect("founded");
    assert!(outputs.unfinished, "the state comes over a part at a time");
    block_on(sim.devices[PHONE].settle(&mut outputs)).expect("settled");

    assert!(!outputs.unfinished);
    let everything = Changed::Everything { group: new };
    let told = outputs.changed.iter().filter(|it| **it == everything);
    assert!(told.count() <= 1);
    sim.absorb(PHONE, outputs);
    let last = sim.committed(PHONE, new, notes(old), "599");
    assert_eq!(last.as_deref(), Some("value"));
}
