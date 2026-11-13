//! Tables: schema and rows in the same timeline, held together by what
//! each transaction requires.

use super::*;
use crate::model::{ColumnId, Table, Value};

/// A table of people, created with a nonce, so that two devices can
/// each create one without knowing of the other's.
pub(super) fn people(cluster: &Cluster, nonce: u8) -> ModelId {
    ModelId::table(genesis(cluster).id(), [nonce; 32])
}

fn name(table: ModelId) -> ColumnId {
    ColumnId::text(table, [1; 32])
}

fn age(table: ModelId) -> ColumnId {
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
    let prefixes = query.storage_prefixes();
    let scanned: Vec<_> = prefixes.flat_map(|prefix| store.scan(prefix)).collect();

    Table::read(&query.results(&scanned))
}

/// The ages in a table, by row key. Empty if there's no table.
fn ages(cluster: &Cluster, device: usize, table: ModelId) -> Vec<(String, Value)> {
    let table_read = read_table(cluster, device, table);
    let rows = table_read.iter().flat_map(Table::rows);
    let row = |row: &crate::Row| {
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
    let prefixes = query.storage_prefixes();
    let scanned: Vec<_> = prefixes.flat_map(|prefix| store.scan(prefix)).collect();
    let held = query.results(&scanned).pop();

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

fn replace(cluster: &mut Cluster, device: usize, table: ModelId, key: &str, years: i64) {
    let agreement = genesis(cluster).id();
    let cells = [(name(table), Value::from(key)), (age(table), years.into())];
    let key = key.to_owned();
    cluster.run(device, |batch| {
        batch
            .transaction(agreement)
            .replace(table, key, cells)
            .commit()
    });
}

fn delete(cluster: &mut Cluster, device: usize, table: ModelId, key: &str) {
    let agreement = genesis(cluster).id();
    let key = key.to_owned();
    cluster.run(device, |batch| {
        batch.transaction(agreement).delete(table, key).commit()
    });
}

#[test]
fn a_row_can_be_replaced_and_deleted() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", 36);
    insert(&mut cluster, PHONE, table, "grace", 85);
    cluster.settle();

    replace(&mut cluster, LAPTOP, table, "ada", 37);
    delete(&mut cluster, NAS, table, "grace");
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let expected = [("ada".to_owned(), Value::Integer(37))];
        assert_eq!(ages(&cluster, device, table), expected, "device {device}");
    }
}

/// The laptop deletes a row while the phone, apart from it, replaces
/// it. Whichever comes first in the timeline, they end up agreeing: a
/// replacement that finds no row doesn't happen.
#[test]
fn replacing_a_deleted_row_doesnt_bring_it_back() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();
    cluster.disconnect_everyone();
    cluster.settle();

    delete(&mut cluster, LAPTOP, table, "ada");
    cluster.settle();
    replace(&mut cluster, PHONE, table, "ada", 37);
    replace(&mut cluster, PHONE, table, "ada", 38);
    cluster.connect_everyone();
    cluster.settle();

    let rows = ages(&cluster, PHONE, table);
    for device in [LAPTOP, NAS, FRIEND] {
        assert_eq!(ages(&cluster, device, table), rows, "device {device}");
    }

    // The phone's second replacement is after the delete in the
    // timeline, wherever its first one fell.
    assert_eq!(rows, []);
}

#[test]
fn an_upsert_writes_the_row_either_way() {
    let mut cluster = household();
    let (agreement, table) = (genesis(&cluster).id(), people(&cluster, 1));
    create(&mut cluster, PHONE, table);
    for years in [36, 37] {
        cluster.run(PHONE, |batch| {
            let cells = [(age(table), Value::Integer(years))];

            batch
                .transaction(agreement)
                .upsert(table, "ada", cells)
                .commit()
        });
    }

    cluster.settle();
    let expected = [("ada".to_owned(), Value::Integer(37))];
    assert_eq!(ages(&cluster, LAPTOP, table), expected);
}

/// Deleting twice is fine, and takes nothing else down with it.
#[test]
fn deleting_a_row_thats_gone_does_nothing() {
    let mut cluster = household();
    let (agreement, table) = (genesis(&cluster).id(), people(&cluster, 1));
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();

    delete(&mut cluster, LAPTOP, table, "ada");
    cluster.run(NAS, |batch| {
        let cells = [(age(table), Value::Integer(85))];
        let transaction = batch.transaction(agreement);

        transaction
            .delete(table, "ada")
            .insert(table, "grace", cells)
            .commit()
    });
    cluster.settle();

    let expected = [("grace".to_owned(), Value::Integer(85))];
    assert_eq!(ages(&cluster, PHONE, table), expected);
}
