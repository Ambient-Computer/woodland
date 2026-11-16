//! Changing every row that matches, when the rows can't be known.

use super::*;
use crate::model::Filter;

fn email(table: ModelId) -> ColumnId {
    ColumnId::text(table, [9; 32]).unique()
}

/// Ada is 36 and Grace is 85, and everyone knows.
fn staffed() -> (Sim, ModelId) {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", 36);
    insert(&mut cluster, PHONE, table, "grace", 85);
    cluster.settle();

    (cluster, table)
}

fn retired(table: ModelId) -> Filter {
    Filter::all().at_least(age(table), 65)
}

fn everywhere(cluster: &Sim, table: ModelId, expected: &[(&str, i64)]) {
    let expected: Vec<_> = expected
        .iter()
        .map(|&(key, years)| (key.to_owned(), Value::Integer(years)))
        .collect();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(ages(cluster, device, table), expected, "device {device}");
    }
}

/// Writes `transaction` along with a note, and says whether the note
/// was committed: whether the transaction happened.
fn happened(cluster: &mut Sim, device: usize, transaction: Transaction) -> bool {
    let notes = household_notes(cluster);
    cluster.write(device, transaction.set(notes, "swept", "yes"));
    cluster.settle();

    read_committed(cluster, NAS, "swept").is_some()
}

#[test]
fn rows_its_author_never_saw_are_deleted_too() {
    let (mut cluster, table) = staffed();
    cluster.isolate(PHONE);
    insert(&mut cluster, LAPTOP, table, "katherine", 101);
    cluster.settle();

    cluster.write(
        PHONE,
        Transaction::new().delete_where(table, retired(table)),
    );
    cluster.connect_all();
    cluster.settle();

    everywhere(&cluster, table, &[("ada", 36)]);
}

#[test]
fn nothing_changes_until_its_committed() {
    let (mut cluster, table) = staffed();
    cluster.disconnect_all();
    cluster.write(
        PHONE,
        Transaction::new().delete_where(table, retired(table)),
    );

    assert_eq!(ages(&cluster, PHONE, table).len(), 2);

    cluster.connect_all();
    cluster.settle();
    everywhere(&cluster, table, &[("ada", 36)]);
}

#[test]
fn rows_that_match_are_changed_and_the_rest_left() {
    let (mut cluster, table) = staffed();
    let younger = Filter::all()
        .below(age(table), 65)
        .is_not(name(table), "nobody");
    let birthday = Transaction::new().update_where(table, younger, [(age(table), 37.into())]);

    assert!(happened(&mut cluster, LAPTOP, birthday));
    everywhere(&cluster, table, &[("ada", 37), ("grace", 85)]);
}

#[test]
fn a_row_added_after_it_was_committed_is_left_alone() {
    let (mut cluster, table) = staffed();
    cluster.write(
        LAPTOP,
        Transaction::new().delete_where(table, retired(table)),
    );
    cluster.settle();
    insert(&mut cluster, PHONE, table, "katherine", 101);
    cluster.settle();

    everywhere(&cluster, table, &[("ada", 36), ("katherine", 101)]);
}

#[test]
fn matching_nothing_changes_nothing_and_still_happens() {
    let (mut cluster, table) = staffed();
    let nobody = Filter::all().above(age(table), 200);

    assert!(happened(
        &mut cluster,
        LAPTOP,
        Transaction::new().delete_where(table, nobody)
    ));
    everywhere(&cluster, table, &[("ada", 36), ("grace", 85)]);
}

#[test]
fn a_change_two_rows_cant_both_take_happens_to_neither() {
    let (mut cluster, table) = staffed();
    cluster.write(
        PHONE,
        Transaction::new().add_column(table, email(table), "email"),
    );
    cluster.settle();

    let shared = [
        (email(table), Value::from("staff@example.com")),
        (age(table), 1.into()),
    ];
    let everyone = Transaction::new().update_where(table, Filter::all(), shared);

    assert!(!happened(&mut cluster, LAPTOP, everyone));
    everywhere(&cluster, table, &[("ada", 36), ("grace", 85)]);
}

#[test]
fn a_row_something_refers_to_stops_the_delete() {
    let (mut cluster, table) = staffed();
    let read = read_table(&cluster, LAPTOP, table).expect("a table");
    let grace = read.row(b"grace").expect("a row");
    cluster.write(LAPTOP, Transaction::new().refer(table, grace));
    cluster.settle();

    let sweep = Transaction::new().delete_where(table, Filter::all());

    assert!(!happened(&mut cluster, PHONE, sweep));
    everywhere(&cluster, table, &[("ada", 36), ("grace", 85)]);
}

#[test]
fn a_test_against_the_wrong_type_of_value_is_refused() {
    let (mut cluster, table) = staffed();
    let confused = Filter::all().is(age(table), "old");

    assert!(!happened(
        &mut cluster,
        PHONE,
        Transaction::new().delete_where(table, confused)
    ));
    everywhere(&cluster, table, &[("ada", 36), ("grace", 85)]);
}

#[test]
fn a_table_that_isnt_there_stops_it() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    let sweep = Transaction::new().delete_where(table, Filter::all());

    assert!(!happened(&mut cluster, PHONE, sweep));
}

#[test]
fn a_column_with_nothing_in_it_is_found_by_asking_for_nothing() {
    let (mut cluster, table) = staffed();
    let cells = [(name(table), Value::from("ageless"))];
    cluster.write(PHONE, Transaction::new().insert(table, "ageless", cells));
    cluster.settle();

    let unknown = Filter::all().is(age(table), Value::Null);
    cluster.write(LAPTOP, Transaction::new().delete_where(table, unknown));
    cluster.settle();

    everywhere(&cluster, table, &[("ada", 36), ("grace", 85)]);
}
