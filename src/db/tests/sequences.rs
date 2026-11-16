//! Ordered lists that several devices edit at once.

use super::*;
use crate::model::Sequence;

const TODO: &str = "todo";

fn lists(sim: &Sim) -> ModelId {
    ModelId::sequence(sim.home, [8; 32])
}

fn list(sim: &Sim, device: usize, committed: bool) -> Sequence {
    let query = Query::key(sim.home, lists(sim), TODO.as_bytes());
    let query = if committed { query.committed() } else { query };

    Sequence::read(&sim.read(device, &query), TODO)
}

fn items(sim: &Sim, device: usize, committed: bool) -> Vec<String> {
    let list = list(sim, device, committed);

    list.values()
        .map(|it| String::from_utf8_lossy(it).into_owned())
        .collect()
}

fn insert(sim: &mut Sim, device: usize, index: usize, values: &[&str]) {
    let read = list(sim, device, false);
    let transaction = Transaction::new().insert_at(lists(sim), TODO, &read, index, values.to_vec());
    sim.write(device, transaction);
}

/// Milk, eggs and flour, committed everywhere.
fn shopping() -> Sim {
    let mut sim = household();
    insert(&mut sim, PHONE, 0, &["milk", "eggs", "flour"]);
    sim.settle();

    sim
}

#[test]
fn a_run_goes_in_where_it_was_put_in_order() {
    let mut sim = shopping();
    assert_eq!(items(&sim, FRIEND, true), ["milk", "eggs", "flour"]);

    insert(&mut sim, LAPTOP, 1, &["tea", "jam"]);
    assert_eq!(
        items(&sim, LAPTOP, false),
        ["milk", "tea", "jam", "eggs", "flour"]
    );
    assert_eq!(items(&sim, LAPTOP, true), ["milk", "eggs", "flour"]);

    insert(&mut sim, LAPTOP, 99, &["salt"]);
    sim.settle();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(
            items(&sim, device, true),
            ["milk", "tea", "jam", "eggs", "flour", "salt"],
            "device {device}"
        );
    }
}

/// Neither takes the other's place, and neither run is split.
#[test]
fn what_devices_put_in_while_apart_is_all_there_when_they_meet() {
    let mut sim = shopping();
    sim.disconnect_all();
    insert(&mut sim, PHONE, 1, &["tea", "jam"]);
    insert(&mut sim, LAPTOP, 1, &["rice", "oats"]);
    sim.connect_all();
    sim.settle();

    let read = items(&sim, PHONE, true);
    let whole = |run: [&str; 2]| read.windows(2).any(|pair| pair == run);
    assert_eq!(read.len(), 7);
    assert_eq!((read[0].as_str(), read[5].as_str()), ("milk", "eggs"));
    assert!(whole(["tea", "jam"]) && whole(["rice", "oats"]), "{read:?}");
    for device in [LAPTOP, NAS, FRIEND] {
        assert_eq!(items(&sim, device, true), read, "device {device}");
        assert_eq!(items(&sim, device, false), read, "device {device}");
    }
}

/// What was read before it was all committed is what's committed.
#[test]
fn committing_changes_nothing_about_how_a_list_reads() {
    let mut sim = shopping();
    sim.disconnect_all();
    let read = list(&sim, LAPTOP, false);
    let lists = lists(&sim);
    sim.write(LAPTOP, Transaction::new().remove_at(lists, TODO, &read, 1));
    insert(&mut sim, PHONE, 2, &["tea"]);
    sim.connect_all();
    sim.run();

    let before = items(&sim, NAS, false);
    assert_eq!(before, ["milk", "tea", "flour"]);
    sim.settle();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(items(&sim, device, true), before, "device {device}");
    }
}

#[test]
fn an_item_moved_on_two_devices_is_in_one_place() {
    let mut sim = shopping();
    sim.disconnect_all();
    let lists = lists(&sim);
    let read = list(&sim, PHONE, false);
    sim.write(PHONE, Transaction::new().move_to(lists, TODO, &read, 0, 2));
    assert_eq!(items(&sim, PHONE, false), ["eggs", "flour", "milk"]);
    sim.write(LAPTOP, Transaction::new().move_to(lists, TODO, &read, 0, 1));
    assert_eq!(items(&sim, LAPTOP, false), ["eggs", "milk", "flour"]);
    sim.write(NAS, Transaction::new().remove_at(lists, TODO, &read, 2));
    sim.connect_all();
    sim.settle();

    let read = items(&sim, PHONE, true);
    assert_eq!(read.len(), 2);
    assert_eq!(read.iter().filter(|it| *it == "milk").count(), 1);
    for device in [LAPTOP, NAS, FRIEND] {
        assert_eq!(items(&sim, device, true), read, "device {device}");
    }
}

/// A list is committed state like any other: it's what a device that
/// missed the history is sent.
#[test]
fn a_list_survives_its_history() {
    let mut sim = shopping();
    let group = sim.home;
    insert(&mut sim, LAPTOP, 3, &["salt"]);
    sim.settle();
    for device in [PHONE, LAPTOP, NAS] {
        sim.compact(device, group);
    }

    sim.forget(FRIEND, group);
    sim.connect_all();
    sim.settle();

    assert_eq!(items(&sim, FRIEND, true), ["milk", "eggs", "flour", "salt"]);
    insert(&mut sim, PHONE, 0, &["tea"]);
    sim.settle();
    assert_eq!(
        items(&sim, FRIEND, true).first().map(String::as_str),
        Some("tea")
    );
}
