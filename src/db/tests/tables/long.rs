//! Transactions that take more than a step to commit.

use std::collections::BTreeSet;

use super::*;
use crate::model::Filter;

const CROWD: usize = 40;

fn email(table: ModelId) -> ColumnId {
    ColumnId::text(table, [9; 32]).unique()
}

/// Forty people, aged nought to thirty-nine, and everyone knows. Then
/// the friend loses touch.
fn crowd() -> (Sim, ModelId) {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    for index in 0..CROWD {
        let years = i64::try_from(index).expect("small");
        insert(&mut cluster, PHONE, table, &format!("p{index:02}"), years);
    }
    cluster.settle();
    cluster.isolate(FRIEND);

    (cluster, table)
}

/// How many rows the device's store has committed, whatever it
/// shows.
fn stored(cluster: &Sim, device: usize, table: ModelId) -> usize {
    let query = Query::table(cluster.home, table).committed();
    let read = Table::read(&cluster.stored(device, &query));

    read.map_or(0, |table| table.rows().len())
}

/// How many rows the device shows as committed.
fn rows(cluster: &Sim, device: usize, table: ModelId) -> usize {
    let query = Query::table(cluster.home, table).committed();
    let read = Table::read(&cluster.read(device, &query));

    read.map_or(0, |table| table.rows().len())
}

/// The sum of everything the device has committed to the table's
/// clock: what a checkpoint's state root is the name of.
fn sum(cluster: &Sim, device: usize, table: ModelId) -> Vec<u8> {
    let key = keys::state(cluster.home, table.clock());
    let (start, end) = (key.clone(), keys::after(&key));
    let view = cluster.devices[device].store.view();
    let mut found = block_on(crate::db::step::scan(&view, &start, &end)).expect("scanned");

    found.pop().expect("a sum").1
}

/// The friend hears of everything at once, and makes a start on it.
fn start(cluster: &mut Sim) {
    let height = cluster.devices[PHONE].groups[&cluster.home].folded();
    cluster.connect(FRIEND, PHONE);
    while cluster.devices[FRIEND].groups[&cluster.home].reached() < height {
        assert!(cluster.deliver(), "it never heard");
    }
    assert!(cluster.devices[FRIEND].groups[&cluster.home].folded() < height);
}

/// How many rows the device shows, and how many of them its index of
/// ages finds.
fn shown(cluster: &Sim, device: usize, table: ModelId) -> (usize, usize) {
    let read = Table::read(&cluster.read(device, &Query::table(cluster.home, table)));
    let indexed = Query::index(cluster.home, table, age(table), ..);

    (
        read.map_or(0, |table| table.rows().len()),
        cluster.read(device, &indexed).len(),
    )
}

/// Stops the friend, starts it again, and has it finish. Returns how
/// many rows it had committed after each step. What it shows, and
/// what its index finds, are one table the whole way.
fn finish(cluster: &mut Sim, table: ModelId) -> BTreeSet<usize> {
    cluster.restart(FRIEND);
    let mut seen = BTreeSet::new();
    while cluster.resume() {
        seen.insert(rows(cluster, FRIEND, table));
        let (rows, indexed) = shown(cluster, FRIEND, table);
        let kept = cluster.devices[FRIEND].has_index(cluster.home, table, age(table));
        assert!(
            !kept || rows == indexed,
            "the index and the rows are of one moment"
        );
    }

    seen
}

#[test]
fn a_change_to_every_row_is_committed_over_many_steps() {
    let (mut cluster, table) = crowd();
    let old = Filter::all().at_least(age(table), 20);
    let cells = [(name(table), Value::from("ada")), (age(table), 1.into())];
    let transaction = Transaction::new()
        .insert(table, "ada", cells)
        .delete_where(table, old);
    cluster.write(PHONE, transaction);
    cluster.settle();
    assert_eq!(rows(&cluster, PHONE, table), 21);
    cluster.keep_index(FRIEND, table, age(table));
    assert_eq!(shown(&cluster, FRIEND, table), (CROWD, CROWD));

    // While it's being worked out, none of it shows.
    start(&mut cluster);
    assert_eq!(rows(&cluster, FRIEND, table), CROWD);

    // Once it's known to happen it's put in place, a part at a time,
    // and still none of it shows.
    while stored(&cluster, FRIEND, table) == CROWD {
        assert!(cluster.resume());
    }
    assert!((22..=CROWD + 1).contains(&stored(&cluster, FRIEND, table)));
    assert_eq!(rows(&cluster, FRIEND, table), CROWD);
    assert!(cluster.devices[FRIEND].groups[&cluster.home].is_busy());

    // Stopped partway through that, it carries on. It doesn't start
    // again on top of the part that's done. A reader sees the table
    // as it was, and then as it is: never some of each.
    let seen = finish(&mut cluster, table);
    assert_eq!(seen, [21, CROWD].into());
    assert_eq!(rows(&cluster, FRIEND, table), 21);
    assert_eq!(shown(&cluster, FRIEND, table), (21, 21));
    assert_eq!(sum(&cluster, FRIEND, table), sum(&cluster, PHONE, table));
}

/// The second is worked out against what the first left, wherever each
/// has got to in the table.
#[test]
fn two_changes_to_every_row_in_one_transaction_both_happen() {
    let (mut cluster, table) = crowd();
    let old = Filter::all().at_least(age(table), 20);
    let young = Filter::all().at_most(age(table), 4);
    let transaction = Transaction::new()
        .delete_where(table, old)
        .delete_where(table, young);
    cluster.write(PHONE, transaction);
    cluster.settle();
    assert_eq!(rows(&cluster, PHONE, table), 15);

    start(&mut cluster);
    finish(&mut cluster, table);

    assert_eq!(rows(&cluster, FRIEND, table), 15);
    assert_eq!(sum(&cluster, FRIEND, table), sum(&cluster, PHONE, table));
}

#[test]
fn a_change_to_every_row_that_fails_late_leaves_nothing() {
    let (mut cluster, table) = crowd();
    let before = sum(&cluster, PHONE, table);
    let notes = household_notes(&cluster);

    // The last two rows would share an address, and can't.
    let last = Filter::all().at_least(age(table), 38);
    let shared = [(email(table), Value::from("all@example.com"))];
    let transaction = Transaction::new()
        .set(notes, "mailed", "yes")
        .update_where(table, last, shared);
    cluster.write(PHONE, transaction);
    cluster.settle();
    assert_eq!(read_committed(&cluster, PHONE, "mailed"), None);

    start(&mut cluster);
    let seen = finish(&mut cluster, table);

    assert_eq!(seen, BTreeSet::from([CROWD]));
    assert_eq!(read_committed(&cluster, FRIEND, "mailed"), None);
    assert_eq!(sum(&cluster, FRIEND, table), before);
    assert_eq!(sum(&cluster, PHONE, table), before);
}

#[test]
fn a_dropped_table_is_emptied_over_many_steps() {
    let (mut cluster, table) = crowd();
    cluster.write(PHONE, Transaction::new().drop_model(table));
    cluster.settle();
    assert_eq!(read_table(&cluster, PHONE, table), None);

    // Its values go a part at a time.
    start(&mut cluster);
    let kept = |cluster: &Sim| {
        let (start, end) = keys::range(keys::committed_value(cluster.home, table, b""));
        let view = cluster.devices[FRIEND].store.view();

        block_on(crate::db::step::scan(&view, &start, &end))
            .expect("scanned")
            .len()
    };
    cluster.restart(FRIEND);
    let mut seen = BTreeSet::new();
    while cluster.resume() {
        seen.insert(kept(&cluster));
    }

    assert!(seen.len() > 3);
    assert_eq!(kept(&cluster), 0);
    assert_eq!(read_table(&cluster, FRIEND, table), None);
    assert_eq!(sum(&cluster, FRIEND, table), sum(&cluster, PHONE, table));
}
