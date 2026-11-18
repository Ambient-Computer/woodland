//! References to blobs, as values in a table. A reference is ordered,
//! judged, and committed like any other value, whoever holds what it
//! names.

use super::*;
use crate::BlobRef;

pub(in crate::db::tests) fn picture(table: ModelId) -> ColumnId {
    ColumnId::blob(table, [3; 32])
}

/// Some bytes, different for each `seed`.
pub(in crate::db::tests) fn bytes(seed: u8, length: usize) -> Vec<u8> {
    let bytes = (0..=250u8).cycle().map(|byte| byte ^ seed);

    bytes.take(length).collect()
}

pub(in crate::db::tests) fn create_with_pictures(cluster: &mut Sim, device: usize, table: ModelId) {
    let columns = [(name(table), "name"), (picture(table), "picture")];
    cluster.write(device, Transaction::new().create_table(table, columns));
}

pub(in crate::db::tests) fn insert_with(
    cluster: &mut Sim,
    device: usize,
    table: ModelId,
    key: &str,
    blob: BlobRef,
) {
    let cells = [
        (name(table), Value::from(key)),
        (picture(table), blob.into()),
    ];
    cluster.write(
        device,
        Transaction::new().insert(table, key.to_owned(), cells),
    );
}

fn pictures(cluster: &Sim, device: usize, table: ModelId, committed: bool) -> Vec<Value> {
    let query = Query::table(cluster.home, table);
    let query = if committed { query.committed() } else { query };
    let read = Table::read(&cluster.reader(device).read(&query));
    let rows = read.iter().flat_map(Table::rows);

    rows.map(|row| row.get(picture(table)).clone()).collect()
}

#[test]
fn a_reference_is_a_value_like_any_other() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create_with_pictures(&mut cluster, PHONE, table);
    let blob = cluster.import(PHONE, &bytes(7, 1000));
    insert_with(&mut cluster, PHONE, table, "ada", blob);
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        for committed in [false, true] {
            let found = pictures(&cluster, device, table, committed);
            assert_eq!(found, [Value::Blob(blob)], "device {device}");
        }
    }
}

#[test]
fn a_reference_is_replaced_like_any_other_value() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create_with_pictures(&mut cluster, PHONE, table);
    let first = cluster.import(PHONE, &bytes(7, 1000));
    insert_with(&mut cluster, PHONE, table, "ada", first);
    cluster.settle();

    let read = read_table(&cluster, LAPTOP, table).expect("a table");
    let row = read.rows().first().expect("a row");
    let second = cluster.import(LAPTOP, &bytes(8, 1000));
    let cells = [(picture(table), Value::from(second))];
    cluster.write(LAPTOP, Transaction::new().update(table, row, cells));
    cluster.settle();

    for device in [PHONE, NAS] {
        let found = pictures(&cluster, device, table, true);
        assert_eq!(found, [Value::Blob(second)], "device {device}");
    }
}

/// A blob says nothing about itself to tell two apart by, so there's
/// nothing for a unique column of them to mean.
#[test]
fn a_column_of_blobs_cant_be_unique() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    let unique = picture(table).unique();
    let columns = [(name(table), "name"), (unique, "picture")];
    cluster.write(PHONE, Transaction::new().create_table(table, columns));
    cluster.settle();

    assert!(read_table(&cluster, PHONE, table).is_none());
}
