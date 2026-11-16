//! Changing a table's columns, while rows are being written.

use super::*;

fn email(table: ModelId) -> ColumnId {
    ColumnId::text(table, [3; 32])
}

/// Writes a row with only an email.
fn write_email(cluster: &mut Sim, device: usize, table: ModelId, key: &str) {
    let cells = [(email(table), Value::from(format!("{key}@example.com")))];
    let key = key.to_owned();
    cluster.write(device, Transaction::new().insert(table, key, cells));
}

fn emails(cluster: &Sim, device: usize, table: ModelId) -> Vec<Value> {
    let read = read_table(cluster, device, table);
    let rows = read.iter().flat_map(Table::rows);

    rows.map(|row| row.get(email(table)).clone()).collect()
}

fn column_names(cluster: &Sim, device: usize, table: ModelId) -> Vec<String> {
    let read = read_table(cluster, device, table);
    let mut names: Vec<_> = read.iter().flat_map(Table::columns).collect();
    names.sort_by_key(|&(_, name)| name);

    names.into_iter().map(|(_, name)| name.to_owned()).collect()
}

/// A table with people's names and ages, known to everyone.
fn table_of_people() -> (Sim, ModelId) {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    cluster.settle();

    (cluster, table)
}

#[test]
fn a_column_can_be_added_and_written_to() {
    let (mut cluster, table) = table_of_people();
    cluster.write(LAPTOP, {
        let transaction = Transaction::new();

        transaction.add_column(table, email(table), "email")
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
    let phone_number = ColumnId::text(table, [4; 32]);
    cluster.disconnect_all();
    cluster.settle();

    cluster.write(PHONE, {
        let transaction = Transaction::new();

        transaction.add_column(table, email(table), "email")
    });
    cluster.write(LAPTOP, {
        let transaction = Transaction::new();

        transaction.add_column(table, phone_number, "phone")
    });
    cluster.connect_all();
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
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();
    cluster.disconnect_all();
    cluster.settle();

    cluster.write(LAPTOP, {
        Transaction::new().drop_column(table, age(table))
    });
    cluster.settle();
    insert(&mut cluster, PHONE, table, "grace", 85);
    insert(&mut cluster, PHONE, table, "alan", 41);
    cluster.connect_all();
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

/// A column is there for everyone until dropping it is committed, the
/// device that dropped it included.
#[test]
fn a_column_isnt_gone_until_dropping_it_is_committed() {
    let (mut cluster, table) = table_of_people();
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();
    cluster.disconnect_all();

    cluster.write(PHONE, Transaction::new().drop_column(table, age(table)));
    cluster.settle();

    assert_eq!(column_names(&cluster, PHONE, table), ["age", "name"]);
    let expected = [("ada".to_owned(), Value::Integer(36))];
    assert_eq!(ages(&cluster, PHONE, table), expected);
    let pending = block_on(cluster.devices[PHONE].pending(cluster.home)).expect("read");
    let dropping = crate::db::Waiting::DropColumn { column: age(table) };
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].change, dropping);

    cluster.connect_all();
    cluster.settle();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(column_names(&cluster, device, table), ["name"]);
    }
}

/// What a row held in a dropped column is still inside the row. If the
/// column could be added again, it would be read again.
#[test]
fn a_dropped_column_cant_come_back() {
    let (mut cluster, table) = table_of_people();
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.write(PHONE, Transaction::new().drop_column(table, age(table)));
    cluster.write(PHONE, {
        let transaction = Transaction::new();

        transaction.add_column(table, age(table), "age")
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
fn a_table_can_be_renamed() {
    let (mut cluster, table) = table_of_people();
    insert(&mut cluster, PHONE, table, "ada", 36);
    let name =
        |cluster: &Sim, device| read_table(cluster, device, table).map(|it| it.name().to_owned());
    assert_eq!(name(&cluster, NAS).as_deref(), Some(""));

    // Two devices rename it at once. The later in the timeline is
    // what's read, everywhere, and its rows are where they were.
    cluster.disconnect_all();
    cluster.write(PHONE, Transaction::new().rename_table(table, "people"));
    cluster.write(LAPTOP, Transaction::new().rename_table(table, "friends"));
    cluster.connect_all();
    cluster.settle();

    let named = name(&cluster, NAS).expect("it exists");
    assert!(named == "people" || named == "friends");
    assert_eq!(name(&cluster, PHONE), Some(named.clone()));
    assert_eq!(name(&cluster, LAPTOP), Some(named));
    assert_eq!(
        ages(&cluster, NAS, table),
        [("ada".to_owned(), Value::Integer(36))]
    );
}

/// A name is given to a table that exists.
#[test]
fn a_table_that_isnt_there_cant_be_named() {
    let mut cluster = household();
    let table = ModelId::table(cluster.home, [7; 32]);
    cluster.write(PHONE, Transaction::new().rename_table(table, "people"));
    cluster.settle();

    assert!(read_table(&cluster, NAS, table).is_none());
}

#[test]
fn a_column_can_be_renamed() {
    let (mut cluster, table) = table_of_people();
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.write(LAPTOP, {
        let transaction = Transaction::new();

        transaction.rename_column(table, age(table), "years")
    });
    cluster.settle();

    assert_eq!(column_names(&cluster, NAS, table), ["name", "years"]);
    assert_eq!(
        ages(&cluster, NAS, table),
        [("ada".to_owned(), Value::Integer(36))]
    );
}
