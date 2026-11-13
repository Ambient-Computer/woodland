//! Changing a table's columns, while rows are being written.

use super::*;

fn email(table: ModelId) -> ColumnId {
    ColumnId::text(table, [3; 32])
}

/// Writes a row with only an email.
fn write_email(cluster: &mut Cluster, device: usize, table: ModelId, key: &str) {
    let agreement = genesis(cluster).id();
    let cells = [(email(table), Value::from(format!("{key}@example.com")))];
    let key = key.to_owned();
    cluster.run(device, |batch| {
        batch
            .transaction(agreement)
            .insert(table, key, cells)
            .commit()
    });
}

fn emails(cluster: &Cluster, device: usize, table: ModelId) -> Vec<Value> {
    let read = read_table(cluster, device, table);
    let rows = read.iter().flat_map(Table::rows);

    rows.map(|row| row.get(email(table)).clone()).collect()
}

fn column_names(cluster: &Cluster, device: usize, table: ModelId) -> Vec<String> {
    let read = read_table(cluster, device, table);
    let mut names: Vec<_> = read.iter().flat_map(Table::columns).collect();
    names.sort_by_key(|&(_, name)| name);

    names.into_iter().map(|(_, name)| name.to_owned()).collect()
}

/// A table with people's names and ages, known to everyone.
fn table_of_people() -> (Cluster, ModelId) {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    cluster.settle();

    (cluster, table)
}

#[test]
fn a_column_can_be_added_and_written_to() {
    let (mut cluster, table) = table_of_people();
    let agreement = genesis(&cluster).id();
    cluster.run(LAPTOP, |batch| {
        let transaction = batch.transaction(agreement);

        transaction
            .add_column(table, email(table), "email")
            .commit()
    });
    cluster.settle();
    write_email(&mut cluster, NAS, table, "ada");
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(
            column_names(&cluster, device, table),
            ["age", "email", "name"]
        );
        assert_eq!(
            emails(&cluster, device, table),
            [Value::from("ada@example.com")]
        );
    }
}

#[test]
fn a_column_that_was_never_added_cant_be_written_to() {
    let (mut cluster, table) = table_of_people();
    write_email(&mut cluster, PHONE, table, "ada");
    cluster.settle();

    assert_eq!(ages(&cluster, LAPTOP, table), []);
}

/// Two devices each add a column while apart. Both columns exist
/// afterwards: neither change overwrote the other.
#[test]
fn columns_added_apart_are_both_kept() {
    let (mut cluster, table) = table_of_people();
    let agreement = genesis(&cluster).id();
    let phone_number = ColumnId::text(table, [4; 32]);
    cluster.disconnect_everyone();
    cluster.settle();

    cluster.run(PHONE, |batch| {
        let transaction = batch.transaction(agreement);

        transaction
            .add_column(table, email(table), "email")
            .commit()
    });
    cluster.run(LAPTOP, |batch| {
        let transaction = batch.transaction(agreement);

        transaction
            .add_column(table, phone_number, "phone")
            .commit()
    });
    cluster.connect_everyone();
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS] {
        let names = column_names(&cluster, device, table);
        assert_eq!(names, ["age", "email", "name", "phone"], "device {device}");
    }
}

/// The laptop drops the age column. The phone, which hasn't heard,
/// keeps writing ages. Its writes are later in the timeline than the
/// drop, so none of them happen. And the ages written before the drop
/// are no longer read.
#[test]
fn writes_to_a_dropped_column_dont_happen() {
    let (mut cluster, table) = table_of_people();
    let agreement = genesis(&cluster).id();
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();
    cluster.disconnect_everyone();
    cluster.settle();

    cluster.run(LAPTOP, |batch| {
        batch
            .transaction(agreement)
            .drop_column(table, age(table))
            .commit()
    });
    cluster.settle();
    insert(&mut cluster, PHONE, table, "grace", 85);
    insert(&mut cluster, PHONE, table, "alan", 41);
    cluster.connect_everyone();
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(column_names(&cluster, device, table), ["name"]);
        let rows = ages(&cluster, device, table);
        assert!(
            rows.iter().all(|(_, age)| age == &Value::Null),
            "device {device}"
        );
        assert!(rows.iter().any(|(key, _)| key == "ada"));
        assert!(rows.iter().all(|(key, _)| key != "alan"));
    }
}

/// What a row held in a dropped column is still inside the row. If the
/// column could be added again, it would be read again.
#[test]
fn a_dropped_column_cant_come_back() {
    let (mut cluster, table) = table_of_people();
    let agreement = genesis(&cluster).id();
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.run(PHONE, |batch| {
        batch
            .transaction(agreement)
            .drop_column(table, age(table))
            .commit()
    });
    cluster.run(PHONE, |batch| {
        let transaction = batch.transaction(agreement);

        transaction.add_column(table, age(table), "age").commit()
    });
    cluster.settle();

    for device in [PHONE, LAPTOP] {
        assert_eq!(column_names(&cluster, device, table), ["name"]);
        assert_eq!(
            ages(&cluster, device, table),
            [("ada".to_owned(), Value::Null)]
        );
    }
}

#[test]
fn a_column_can_be_renamed() {
    let (mut cluster, table) = table_of_people();
    let agreement = genesis(&cluster).id();
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.run(LAPTOP, |batch| {
        let transaction = batch.transaction(agreement);

        transaction
            .rename_column(table, age(table), "years")
            .commit()
    });
    cluster.settle();

    assert_eq!(column_names(&cluster, NAS, table), ["name", "years"]);
    assert_eq!(
        ages(&cluster, NAS, table),
        [("ada".to_owned(), Value::Integer(36))]
    );
}
