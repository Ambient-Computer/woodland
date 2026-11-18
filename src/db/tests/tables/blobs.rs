//! References to blobs, as values in a table. Nothing here has any
//! bytes: a reference is ordered, judged, and committed like any other
//! value, whoever holds what it names.

use super::*;
use crate::BlobRef;

fn picture(table: ModelId) -> ColumnId {
    ColumnId::blob(table, [3; 32])
}

fn blob(name: u8) -> BlobRef {
    BlobRef::new([name; 32], 1000, None)
}

fn create_with_pictures(cluster: &mut Sim, device: usize, table: ModelId) {
    let columns = [(name(table), "name"), (picture(table), "picture")];
    cluster.write(device, Transaction::new().create_table(table, columns));
}

fn insert_with(cluster: &mut Sim, device: usize, table: ModelId, key: &str, blob: BlobRef) {
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
    insert_with(&mut cluster, PHONE, table, "ada", blob(7));
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        for committed in [false, true] {
            let found = pictures(&cluster, device, table, committed);
            assert_eq!(found, [Value::Blob(blob(7))], "device {device}");
        }
    }
}

#[test]
fn a_reference_is_replaced_like_any_other_value() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create_with_pictures(&mut cluster, PHONE, table);
    insert_with(&mut cluster, PHONE, table, "ada", blob(7));
    cluster.settle();

    let read = read_table(&cluster, LAPTOP, table).expect("a table");
    let row = read.rows().first().expect("a row");
    let cells = [(picture(table), Value::from(blob(8)))];
    cluster.write(LAPTOP, Transaction::new().update(table, row, cells));
    cluster.settle();

    for device in [PHONE, NAS] {
        let found = pictures(&cluster, device, table, true);
        assert_eq!(found, [Value::Blob(blob(8))], "device {device}");
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

/// A sealed group's blobs are encrypted and an open group's aren't.
#[test]
fn a_reference_made_for_a_sealed_group_isnt_written_to_an_open_one() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create_with_pictures(&mut cluster, PHONE, table);
    let sealed = BlobRef::new([7; 32], 1000, Some([9; 32]));
    let cells = [(picture(table), Value::from(sealed))];
    let transaction = Transaction::new().insert(table, "ada", cells);

    let written = block_on(cluster.devices[PHONE].write(cluster.home, transaction));

    assert!(matches!(written, Err(Error::BlobMisplaced)));
}
