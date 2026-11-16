//! Devices on different versions of the library, in one group. The
//! later version has allowances. The earlier one doesn't.

use super::*;
use crate::model::Allowances;
use crate::{ColumnId, Table};

const SEATS: &str = "seats";

fn photos(group: GroupId) -> ClockId {
    ClockId::new(group, [7; 32])
}

/// Seats, in a clock of their own.
fn stock(sim: &Sim) -> ModelId {
    ModelId::allowances(photos(sim.home), [6; 32])
}

fn seats(sim: &Sim, device: usize, committed: bool) -> Option<u64> {
    sim.wake(device);
    let query = Query::prefix(sim.home, stock(sim), b"");
    let query = if committed { query.committed() } else { query };

    Allowances::read(&sim.read(device, &query)).total(SEATS)
}

fn open_seats(sim: &mut Sim, device: usize) {
    sim.wake(device);
    let (model, owner) = (stock(sim), sim.id(device));
    let transaction = Transaction::new().open_allowance(model, SEATS, 10, owner);
    sim.write(device, transaction);
}

/// A household where only some devices have been updated.
fn household_with_old(old: &[usize]) -> Sim {
    let mut sim = household();
    sim.old = old.iter().copied().collect();
    for &device in old {
        sim.restart(device);
    }
    sim.connect_all();
    sim.settle();

    sim
}

fn update(sim: &mut Sim, device: usize) {
    sim.old.remove(&device);
    sim.restart(device);
}

/// The NAS can't tell what the phone wrote. The friend, who hears of
/// it only from the NAS, can.
#[test]
fn what_a_device_cant_make_sense_of_it_keeps_and_passes_on() {
    let mut sim = household_with_old(&[NAS]);
    sim.disconnect_all();
    sim.connect(PHONE, NAS);
    sim.connect(NAS, FRIEND);
    open_seats(&mut sim, PHONE);
    sim.run();

    assert_eq!(seats(&sim, NAS, false), None);
    assert_eq!(sim.entries_kept(NAS, sim.home), 1);
    assert_eq!(seats(&sim, FRIEND, false), Some(10));
}

/// Enough of the group has the kind to commit it. The NAS takes their
/// word that it's committed, and for nothing about what it says.
#[test]
fn a_device_left_behind_catches_up_once_its_updated() {
    let mut sim = household_with_old(&[NAS]);
    let group = sim.home;
    open_seats(&mut sim, PHONE);
    sim.settle();

    assert_eq!(seats(&sim, PHONE, true), Some(10));
    assert_eq!(seats(&sim, NAS, true), None);
    let folded = |sim: &Sim, device: usize| sim.devices[device].groups[&group].folded();
    assert_eq!(folded(&sim, NAS), folded(&sim, PHONE));

    // It still has a say in everything else.
    sim.isolate(LAPTOP);
    sim.write(PHONE, set(notes(group), "todo", "milk"));
    sim.settle();
    assert_eq!(
        sim.committed(NAS, group, notes(group), "todo").as_deref(),
        Some("milk")
    );

    // Starting again changes nothing, for as long as it's the same
    // version that starts.
    sim.restart(NAS);
    assert_eq!(
        sim.committed(NAS, group, notes(group), "todo").as_deref(),
        Some("milk")
    );

    update(&mut sim, NAS);
    assert_eq!(sim.committed(NAS, group, notes(group), "todo"), None);
    sim.connect_all();
    sim.settle();

    assert_eq!(seats(&sim, NAS, true), Some(10));
    assert_eq!(
        sim.committed(NAS, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
}

/// Too few have the kind to commit it. It waits, and holds nothing
/// else up.
#[test]
fn what_most_of_a_group_cant_make_sense_of_waits_for_them() {
    let mut sim = household_with_old(&[LAPTOP, NAS]);
    let group = sim.home;
    open_seats(&mut sim, PHONE);
    sim.write(LAPTOP, set(notes(group), "todo", "milk"));
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(seats(&sim, device, true), None, "device {device}");
        assert_eq!(
            sim.committed(device, group, notes(group), "todo")
                .as_deref(),
            Some("milk"),
            "device {device}"
        );
    }
    assert_eq!(seats(&sim, PHONE, false), Some(10));

    // Nothing was committed unread, so there's nothing to fetch again.
    update(&mut sim, LAPTOP);
    assert_eq!(sim.entries_kept(LAPTOP, group), 2);
    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP] {
        assert_eq!(seats(&sim, device, true), Some(10), "device {device}");
    }
}

/// Whoever wrote what nobody else can read can be removed, and then
/// it's as if they hadn't.
#[test]
fn a_clock_nobody_can_read_carries_on_once_its_writer_is_gone() {
    let mut sim = household_with_old(&[LAPTOP, NAS]);
    let group = sim.home;
    let captions = ModelId::kv(photos(group), [1; 32]);
    open_seats(&mut sim, PHONE);
    sim.settle();
    let refused = block_on(sim.devices[LAPTOP].write(group, set(captions, "beach", "sunset")));
    assert!(matches!(refused, Err(Error::Unreadable)));

    sim.reconfigure(LAPTOP, group, [LAPTOP, NAS]);
    sim.settle();
    sim.write(LAPTOP, set(captions, "beach", "sunset"));
    sim.settle();

    for device in [LAPTOP, NAS, FRIEND] {
        assert_eq!(
            sim.committed(device, group, captions, "beach").as_deref(),
            Some("sunset"),
            "device {device}"
        );
    }
}

/// The NAS has tables, from before a table could be named. A rename
/// isn't something it can throw away: it stops vouching for the clock
/// until it can read it.
#[test]
fn an_operation_added_to_a_kind_since_waits_for_an_update() {
    let mut sim = household_with_old(&[NAS]);
    let group = sim.home;
    let table = ModelId::table(photos(group), [9; 32]);
    let columns: [(ColumnId, &str); 0] = [];
    sim.write(PHONE, Transaction::new().create_table(table, columns));
    sim.settle();
    sim.write(PHONE, Transaction::new().rename_table(table, "people"));
    sim.settle();

    let name = |sim: &Sim, device: usize| {
        sim.wake(device);
        let read = Table::read(&sim.read(device, &Query::table(group, table).committed()));

        read.map(|it| it.name().to_owned())
    };
    assert_eq!(name(&sim, PHONE).as_deref(), Some("people"));
    assert_ne!(name(&sim, NAS).as_deref(), Some("people"));

    update(&mut sim, NAS);
    sim.connect_all();
    sim.settle();

    assert_eq!(name(&sim, NAS).as_deref(), Some("people"));
}
