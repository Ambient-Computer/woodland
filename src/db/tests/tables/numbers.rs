//! Numbering rows: a sequence with no gaps, given out at commit.

use super::*;

/// Invoices, numbered in the order they became final.
fn invoices(cluster: &Sim) -> ModelId {
    ModelId::table(cluster.home, [8; 32])
}

fn number(table: ModelId) -> ColumnId {
    ColumnId::integer(table, [1; 32])
}

fn memo(table: ModelId) -> ColumnId {
    ColumnId::text(table, [2; 32])
}

fn billing() -> (Sim, ModelId) {
    let mut cluster = household();
    let table = invoices(&cluster);
    let columns = [(number(table), "number"), (memo(table), "memo")];
    cluster.write(PHONE, Transaction::new().create_table(table, columns));
    cluster.settle();

    (cluster, table)
}

fn invoice(cluster: &mut Sim, device: usize, table: ModelId, key: &str) {
    let cells = [(memo(table), Value::from(key))];
    let numbered = Transaction::new().insert_numbered(table, key, cells, number(table));
    cluster.write(device, numbered);
}

/// The number each row holds in `column`, by row key.
fn numbered(
    cluster: &Sim,
    device: usize,
    table: ModelId,
    column: ColumnId,
    committed: bool,
) -> Vec<(String, Value)> {
    let query = Query::table(cluster.home, table);
    let query = if committed { query.committed() } else { query };
    let read = Table::read(&cluster.read(device, &query));
    let row = |row: &Row| {
        let key = String::from_utf8_lossy(row.key()).into_owned();

        (key, row.get(column).clone())
    };

    read.iter().flat_map(Table::rows).map(row).collect()
}

fn numbers(cluster: &Sim, device: usize, table: ModelId) -> Vec<(String, Value)> {
    numbered(cluster, device, table, number(table), true)
}

fn row(cluster: &Sim, device: usize, table: ModelId, key: &str) -> Row {
    let read = read_table(cluster, device, table).expect("a table");

    read.row(key.as_bytes()).expect("a row").clone()
}

#[test]
fn a_row_has_no_number_until_its_committed() {
    let (mut cluster, table) = billing();
    cluster.disconnect_all();
    invoice(&mut cluster, PHONE, table, "first");

    let shown = numbered(&cluster, PHONE, table, number(table), false);
    assert_eq!(shown, [("first".to_owned(), Value::Null)]);
    assert_eq!(numbers(&cluster, PHONE, table), []);

    cluster.connect_all();
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let expected = [("first".to_owned(), Value::Integer(1))];
        assert_eq!(numbers(&cluster, device, table), expected);
        let shown = numbered(&cluster, device, table, number(table), false);
        assert_eq!(shown, expected, "what's committed is shown");
    }
}

#[test]
fn rows_written_apart_are_numbered_without_gaps() {
    let (mut cluster, table) = billing();
    cluster.disconnect_all();
    for (device, key) in [(PHONE, "a"), (LAPTOP, "b"), (PHONE, "c"), (NAS, "d")] {
        invoice(&mut cluster, device, table, key);
    }
    cluster.connect_all();
    cluster.settle();
    invoice(&mut cluster, LAPTOP, table, "e");
    cluster.settle();

    let expected = numbers(&cluster, PHONE, table);
    let mut given: Vec<_> = expected.iter().map(|(_, number)| number.clone()).collect();
    given.sort_by_key(|number| match number {
        Value::Integer(number) => *number,
        _ => 0,
    });
    assert_eq!(given, [1, 2, 3, 4, 5].map(Value::Integer));
    assert!(expected.contains(&("e".to_owned(), Value::Integer(5))));
    for device in [LAPTOP, NAS, FRIEND] {
        assert_eq!(
            numbers(&cluster, device, table),
            expected,
            "device {device}"
        );
    }
}

#[test]
fn a_row_that_never_happened_uses_no_number() {
    let (mut cluster, table) = billing();
    cluster.disconnect_all();
    invoice(&mut cluster, PHONE, table, "same");
    invoice(&mut cluster, LAPTOP, table, "same");
    cluster.connect_all();
    cluster.settle();
    invoice(&mut cluster, NAS, table, "next");
    cluster.settle();

    let expected = [
        ("next".to_owned(), Value::Integer(2)),
        ("same".to_owned(), Value::Integer(1)),
    ];
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(
            numbers(&cluster, device, table),
            expected,
            "device {device}"
        );
    }
}

#[test]
fn a_row_is_numbered_once() {
    let (mut cluster, table) = billing();
    let notes = household_notes(&cluster);
    invoice(&mut cluster, PHONE, table, "first");
    cluster.settle();

    let first = row(&cluster, LAPTOP, table, "first");
    let again = Transaction::new().number(table, &first, number(table));
    cluster.write(LAPTOP, again.set(notes, "renumbered", "yes"));
    cluster.settle();
    invoice(&mut cluster, PHONE, table, "second");
    cluster.settle();

    let expected = [
        ("first".to_owned(), Value::Integer(1)),
        ("second".to_owned(), Value::Integer(2)),
    ];
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(
            numbers(&cluster, device, table),
            expected,
            "device {device}"
        );
        assert_eq!(read(&cluster, device, "renumbered"), None, "it all goes");
    }
}

#[test]
fn a_row_already_there_can_be_numbered() {
    let (mut cluster, table) = billing();
    let cells = [(memo(table), Value::from("draft"))];
    cluster.write(PHONE, Transaction::new().insert(table, "draft", cells));
    cluster.settle();

    let draft = row(&cluster, LAPTOP, table, "draft");
    let issue = Transaction::new().number(table, &draft, number(table));
    cluster.write(LAPTOP, issue);
    cluster.settle();

    let expected = [("draft".to_owned(), Value::Integer(1))];
    assert_eq!(numbers(&cluster, NAS, table), expected);
}

#[test]
fn a_number_meant_for_a_row_thats_gone_isnt_given_to_the_next() {
    let (mut cluster, table) = billing();
    let cells = || [(memo(table), Value::from("draft"))];
    cluster.write(PHONE, Transaction::new().insert(table, "draft", cells()));
    cluster.settle();

    let draft = row(&cluster, LAPTOP, table, "draft");
    cluster.disconnect_all();
    let replace = Transaction::new().delete_row(table, &draft);
    cluster.write(PHONE, replace.insert(table, "draft", cells()));

    // The laptop writes something first, so that numbering the row
    // comes after the phone replaced it.
    let notes = household_notes(&cluster);
    cluster.write(LAPTOP, Transaction::new().set(notes, "busy", "yes"));
    let issue = Transaction::new().number(table, &draft, number(table));
    cluster.write(LAPTOP, issue);
    cluster.connect_all();
    cluster.settle();

    let expected = [("draft".to_owned(), Value::Null)];
    assert_eq!(numbers(&cluster, NAS, table), expected);
}

#[test]
fn only_a_column_of_whole_numbers_counts() {
    let (mut cluster, table) = billing();
    let numbered = Transaction::new().insert_numbered(table, "first", [], memo(table));
    cluster.write(PHONE, numbered);
    cluster.settle();

    assert_eq!(numbers(&cluster, LAPTOP, table), []);
}

#[test]
fn a_column_thats_gone_numbers_nothing() {
    let (mut cluster, table) = billing();
    cluster.write(PHONE, Transaction::new().drop_column(table, number(table)));
    cluster.settle();

    invoice(&mut cluster, LAPTOP, table, "first");
    cluster.settle();

    assert_eq!(numbers(&cluster, NAS, table), [], "the row goes with it");
}

#[test]
fn a_unique_column_can_be_numbered_and_looked_up() {
    let mut cluster = household();
    let table = invoices(&cluster);
    let serial = number(table).unique();
    let columns = [(serial, "serial"), (memo(table), "memo")];
    cluster.write(PHONE, Transaction::new().create_table(table, columns));
    cluster.settle();
    for key in ["a", "b"] {
        let cells = [(memo(table), Value::from(key))];
        let numbered = Transaction::new().insert_numbered(table, key, cells, serial);
        cluster.write(LAPTOP, numbered);
    }
    cluster.settle();

    let expected = [
        ("a".to_owned(), Value::Integer(1)),
        ("b".to_owned(), Value::Integer(2)),
    ];
    assert_eq!(numbered(&cluster, NAS, table, serial, true), expected);
    let second = Query::lookup(cluster.home, table, serial, &Value::Integer(2));
    let found = cluster.read(NAS, &second.committed());
    assert_eq!(found.len(), 1, "found by its number");

    // And only once, like any other.
    let first = row(&cluster, LAPTOP, table, "a");
    cluster.write(LAPTOP, Transaction::new().number(table, &first, serial));
    cluster.settle();
    assert_eq!(numbered(&cluster, NAS, table, serial, true), expected);
}

#[test]
fn a_device_that_caught_up_from_a_snapshot_counts_on_from_there() {
    let (mut cluster, table) = billing();
    cluster.isolate(NAS);
    invoice(&mut cluster, PHONE, table, "a");
    invoice(&mut cluster, LAPTOP, table, "b");
    cluster.settle();
    for device in [PHONE, LAPTOP] {
        cluster.compact(device, cluster.home);
    }
    cluster.connect_all();
    cluster.settle();

    invoice(&mut cluster, NAS, table, "c");
    cluster.settle();

    for device in [PHONE, NAS, FRIEND] {
        let given = numbers(&cluster, device, table);
        assert!(given.contains(&("c".to_owned(), Value::Integer(3))));
    }
}
