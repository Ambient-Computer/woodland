//! Tables: schema and rows in the same timeline, held together by what
//! each transaction requires.

mod indexes;
pub(super) mod references;
mod schema;
pub(super) mod unique;

use super::*;
use crate::agreement::AgreementId;
use crate::model::{ColumnId, Row, Table, Value};

/// A table of people, created with a nonce, so that two devices can
/// each create one without knowing of the other's.
pub(super) fn people(cluster: &Cluster, nonce: u8) -> ModelId {
    ModelId::table(genesis(cluster).id(), [nonce; 32])
}

pub(super) fn name(table: ModelId) -> ColumnId {
    ColumnId::text(table, [1; 32])
}

pub(super) fn age(table: ModelId) -> ColumnId {
    ColumnId::integer(table, [2; 32])
}

/// Creates a table of people, and takes the name "people" for it, in
/// one transaction. Only one table can have the name.
pub(super) fn create(cluster: &mut Cluster, device: usize, table: ModelId) {
    let (agreement, names) = (genesis(cluster).id(), names(cluster));
    cluster.run(device, |batch| {
        let transaction = batch.transaction(agreement);
        let columns = [(name(table), "name"), (age(table), "age")];

        transaction
            .claim(names, "people", table.as_bytes().to_vec())
            .create_table(table, columns)
            .commit()
    });
}

pub(super) fn insert(cluster: &mut Cluster, device: usize, table: ModelId, key: &str, years: i64) {
    let agreement = genesis(cluster).id();
    let cells = [(name(table), Value::from(key)), (age(table), years.into())];
    let key = key.to_owned();
    cluster.run(device, |batch| {
        batch
            .transaction(agreement)
            .insert(table, key, cells)
            .commit()
    });
}

pub(super) fn read_table(cluster: &Cluster, device: usize, table: ModelId) -> Option<Table> {
    let query = Query::table(genesis(cluster).id(), table);
    let store = &cluster.devices[device].store;

    Table::read(&store.read(&query))
}

/// The ages in a table, by row key. Empty if there's no table.
fn ages(cluster: &Cluster, device: usize, table: ModelId) -> Vec<(String, Value)> {
    let table_read = read_table(cluster, device, table);
    let rows = table_read.iter().flat_map(Table::rows);
    let row = |row: &Row| {
        let key = String::from_utf8_lossy(row.key()).into_owned();

        (key, row.get(age(table)).clone())
    };

    rows.map(row).collect()
}

#[test]
fn a_table_and_its_rows_reach_everyone() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();
    insert(&mut cluster, LAPTOP, table, "grace", 85);
    cluster.settle();

    let expected = [
        ("ada".to_owned(), Value::Integer(36)),
        ("grace".to_owned(), Value::Integer(85)),
    ];
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(ages(&cluster, device, table), expected, "device {device}");
        let read = read_table(&cluster, device, table);
        let mut columns: Vec<_> = read.iter().flat_map(Table::columns).collect();
        columns.sort_by_key(|&(_, name)| name);
        assert_eq!(columns, [(age(table), "age"), (name(table), "name")]);
    }
}

#[test]
fn a_row_cant_go_in_a_table_that_doesnt_exist() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();

    // Creating it afterwards doesn't bring the row back: the insert
    // came first in the timeline, and found no table.
    create(&mut cluster, PHONE, table);
    cluster.settle();

    for device in [PHONE, LAPTOP] {
        assert!(read_table(&cluster, device, table).is_some());
        assert_eq!(ages(&cluster, device, table), []);
    }
}

/// The phone and the laptop each create a "people" table while apart,
/// and fill it. Each sees its own until they meet. Only one of them
/// gets the name, and the other's table never existed: not the table,
/// and not one of the rows written to it.
#[test]
fn rows_go_with_the_table_they_were_written_to() {
    let mut cluster = household();
    cluster.disconnect_everyone();
    cluster.settle();
    let (ours, theirs) = (people(&cluster, 1), people(&cluster, 2));
    create(&mut cluster, PHONE, ours);
    create(&mut cluster, LAPTOP, theirs);
    for (key, years) in [("ada", 36), ("grace", 85), ("alan", 41), ("edsger", 72)] {
        insert(&mut cluster, PHONE, ours, key, years);
        insert(&mut cluster, LAPTOP, theirs, key, years + 1);
    }

    cluster.settle();
    assert_eq!(ages(&cluster, PHONE, ours).len(), 4);
    assert_eq!(ages(&cluster, LAPTOP, theirs).len(), 4);

    cluster.connect_everyone();
    cluster.settle();

    let named = holder_bytes(&cluster, PHONE, "people");
    let (winner, loser) = if named == ours.as_bytes() {
        (ours, theirs)
    } else {
        (theirs, ours)
    };

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(ages(&cluster, device, winner).len(), 4, "device {device}");
        assert_eq!(read_table(&cluster, device, loser), None, "device {device}");
    }

    // Nothing of the losing table is left anywhere, once it's settled.
    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(stored_log_len(&cluster, device), 0);
        assert_eq!(stored_frontier(&cluster, device), []);
    }
}

pub(super) fn holder_bytes(cluster: &Cluster, device: usize, name: &str) -> Vec<u8> {
    let query = Query::key(genesis(cluster).id(), names(cluster), name.as_bytes());
    let store = &cluster.devices[device].store;
    let held = store.read(&query).pop();

    held.map(|(_, value)| value).unwrap_or_default()
}

#[test]
fn the_first_row_with_a_key_keeps_it() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    cluster.settle();
    cluster.disconnect_everyone();
    cluster.settle();

    insert(&mut cluster, PHONE, table, "ada", 36);
    insert(&mut cluster, LAPTOP, table, "ada", 99);
    cluster.connect_everyone();
    cluster.settle();

    let rows = ages(&cluster, PHONE, table);
    assert_eq!(rows.len(), 1);
    for device in [LAPTOP, NAS, FRIEND] {
        assert_eq!(ages(&cluster, device, table), rows, "device {device}");
    }
}

#[test]
fn a_value_of_the_wrong_type_is_refused() {
    let mut cluster = household();
    let (agreement, table) = (genesis(&cluster).id(), people(&cluster, 1));
    create(&mut cluster, PHONE, table);
    cluster.run(PHONE, |batch| {
        let cells = [(age(table), Value::from("thirty-six"))];

        batch
            .transaction(agreement)
            .insert(table, "ada", cells)
            .commit()
    });
    cluster.settle();

    assert_eq!(ages(&cluster, LAPTOP, table), []);
}

/// A transaction can create a table and fill it. What it requires is
/// checked against its own earlier operations too.
#[test]
fn a_table_can_be_created_and_filled_at_once() {
    let mut cluster = household();
    let (agreement, table) = (genesis(&cluster).id(), people(&cluster, 1));
    cluster.run(PHONE, |batch| {
        let transaction = batch.transaction(agreement);
        let columns = [(name(table), "name"), (age(table), "age")];

        transaction
            .create_table(table, columns)
            .insert(table, "ada", [(age(table), Value::Integer(36))])
            .insert(table, "grace", [(age(table), Value::Null)])
            .commit()
    });
    cluster.settle();

    let expected = [
        ("ada".to_owned(), Value::Integer(36)),
        ("grace".to_owned(), Value::Null),
    ];
    assert_eq!(ages(&cluster, NAS, table), expected);
}

#[test]
fn a_dropped_table_takes_its_rows() {
    let mut cluster = household();
    let (agreement, table) = (genesis(&cluster).id(), people(&cluster, 1));
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();

    cluster.run(LAPTOP, |batch| {
        batch.transaction(agreement).drop_model(table).commit()
    });
    insert(&mut cluster, PHONE, table, "grace", 85);
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(read_table(&cluster, device, table), None, "device {device}");
    }
}

pub(super) fn row(cluster: &Cluster, device: usize, table: ModelId, key: &str) -> Option<Row> {
    let table = read_table(cluster, device, table)?;

    table.row(key.as_bytes()).cloned()
}

/// Changes one column of a row, as the device reads it now. Does
/// nothing if the device has no such row.
pub(super) fn set(
    cluster: &mut Cluster,
    device: usize,
    table: ModelId,
    key: &str,
    change: (ColumnId, Value),
) {
    let agreement = genesis(cluster).id();
    let Some(row) = row(cluster, device, table, key) else {
        return;
    };

    cluster.run(device, |batch| {
        batch
            .transaction(agreement)
            .update(table, &row, [change])
            .commit()
    });
}

/// Deletes a row, as the device reads it now.
pub(super) fn delete(cluster: &mut Cluster, device: usize, table: ModelId, key: &str) {
    let agreement = genesis(cluster).id();
    let Some(row) = row(cluster, device, table, key) else {
        return;
    };

    cluster.run(device, |batch| {
        batch
            .transaction(agreement)
            .delete_row(table, &row)
            .commit()
    });
}

/// A row's name and age.
fn person(cluster: &Cluster, device: usize, table: ModelId, key: &str) -> Option<(Value, Value)> {
    let row = row(cluster, device, table, key)?;

    Some((row.get(name(table)).clone(), row.get(age(table)).clone()))
}

#[test]
fn a_row_can_be_changed_and_deleted() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", 36);
    insert(&mut cluster, PHONE, table, "grace", 85);
    cluster.settle();

    set(&mut cluster, LAPTOP, table, "ada", (age(table), 37.into()));
    delete(&mut cluster, NAS, table, "grace");
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let expected = [("ada".to_owned(), Value::Integer(37))];
        assert_eq!(ages(&cluster, device, table), expected, "device {device}");
        let ada = person(&cluster, device, table, "ada");
        assert_eq!(ada, Some(("ada".into(), 37.into())), "device {device}");
        assert_eq!(stored_log_len(&cluster, device), 0, "device {device}");
    }
}

/// The phone changes a row's age while the laptop, apart from it,
/// changes its name. Neither wrote over the other: a column is its
/// own key.
#[test]
fn changes_to_different_columns_both_happen() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();
    cluster.disconnect_everyone();
    cluster.settle();

    set(&mut cluster, PHONE, table, "ada", (age(table), 37.into()));
    set(
        &mut cluster,
        LAPTOP,
        table,
        "ada",
        (name(table), "Ada".into()),
    );
    cluster.settle();
    let apart = person(&cluster, PHONE, table, "ada");
    assert_eq!(apart, Some(("ada".into(), 37.into())));

    cluster.connect_everyone();
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let ada = person(&cluster, device, table, "ada");
        assert_eq!(ada, Some(("Ada".into(), 37.into())), "device {device}");
        assert_eq!(stored_frontier(&cluster, device), [], "device {device}");
    }
}

#[test]
fn of_two_changes_to_one_column_the_later_is_read() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();
    cluster.disconnect_everyone();
    cluster.settle();

    set(&mut cluster, PHONE, table, "ada", (age(table), 37.into()));
    set(&mut cluster, LAPTOP, table, "ada", (age(table), 38.into()));
    cluster.connect_everyone();
    cluster.settle();

    let ada = person(&cluster, PHONE, table, "ada");
    let (_, years) = ada.clone().expect("the row exists");
    assert!([Value::Integer(37), Value::Integer(38)].contains(&years));
    for device in [LAPTOP, NAS, FRIEND] {
        assert_eq!(person(&cluster, device, table, "ada"), ada);
    }
}

#[test]
fn a_cell_can_be_cleared() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();

    set(
        &mut cluster,
        LAPTOP,
        table,
        "ada",
        (age(table), Value::Null),
    );
    cluster.settle();

    let ada = person(&cluster, NAS, table, "ada");
    assert_eq!(ada, Some(("ada".into(), Value::Null)));
}

/// The laptop deletes a row while the phone, apart from it, changes
/// it. Whichever comes first in the timeline, the row is gone: a
/// change that finds no row doesn't happen, and a delete doesn't mind
/// what was changed.
#[test]
fn changing_a_deleted_row_doesnt_bring_it_back() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();
    cluster.disconnect_everyone();
    cluster.settle();

    delete(&mut cluster, LAPTOP, table, "ada");
    set(&mut cluster, PHONE, table, "ada", (age(table), 37.into()));
    set(&mut cluster, PHONE, table, "ada", (age(table), 38.into()));
    cluster.connect_everyone();
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(ages(&cluster, device, table), [], "device {device}");
        assert_eq!(stored_frontier(&cluster, device), [], "device {device}");
    }
}

/// A row is deleted, and another is inserted under its key. A change
/// written for the first one, by a device that hadn't heard, falls
/// before the delete in the timeline. It was for a row that's gone,
/// and the new one doesn't get it.
#[test]
fn a_new_row_doesnt_inherit_from_the_last_one_with_its_key() {
    let mut cluster = household();
    let (agreement, table) = (genesis(&cluster).id(), people(&cluster, 1));
    create(&mut cluster, PHONE, table);
    cluster.run(PHONE, |batch| {
        let cells = [(name(table), Value::from("ada"))];

        batch
            .transaction(agreement)
            .insert(table, "ada", cells)
            .commit()
    });
    cluster.settle();
    cluster.disconnect_everyone();
    cluster.settle();

    // The phone's change is written first. The laptop's delete comes
    // after a few other writes, so it's later in the timeline.
    set(&mut cluster, PHONE, table, "ada", (age(table), 36.into()));
    for value in ["one", "two", "three"] {
        write(&mut cluster, LAPTOP, "busy", value);
    }

    delete(&mut cluster, LAPTOP, table, "ada");
    cluster.run(LAPTOP, |batch| {
        let cells = [(name(table), Value::from("Ada"))];

        batch
            .transaction(agreement)
            .insert(table, "ada", cells)
            .commit()
    });
    cluster.connect_everyone();
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let ada = person(&cluster, device, table, "ada");
        assert_eq!(ada, Some(("Ada".into(), Value::Null)), "device {device}");
        assert_eq!(stored_frontier(&cluster, device), [], "device {device}");
    }
}

/// Two devices delete the same row while apart. It's gone either way.
#[test]
fn a_row_deleted_twice_is_gone() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", 36);
    insert(&mut cluster, PHONE, table, "grace", 85);
    cluster.settle();
    cluster.disconnect_everyone();
    cluster.settle();

    delete(&mut cluster, LAPTOP, table, "ada");
    delete(&mut cluster, NAS, table, "ada");
    cluster.connect_everyone();
    cluster.settle();

    let expected = [("grace".to_owned(), Value::Integer(85))];
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(ages(&cluster, device, table), expected, "device {device}");
        assert_eq!(stored_log_len(&cluster, device), 0, "device {device}");
    }
}
