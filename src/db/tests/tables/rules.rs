//! Conditions every row of a table has to meet, once they're agreed.

use super::*;
use crate::db::Waiting;
use crate::model::Filter;

/// Nobody is younger than nothing.
fn born(table: ModelId) -> Filter {
    Filter::all().at_least(age(table), 0)
}

/// Ada is 36, everyone knows, and everyone has agreed that ages
/// aren't negative.
fn ruled() -> (Sim, ModelId) {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.write(PHONE, Transaction::new().add_rule(table, born(table)));
    cluster.settle();

    (cluster, table)
}

/// Writes `transaction` along with a note, and says whether the note
/// was committed: whether the transaction happened.
fn happened(cluster: &mut Sim, device: usize, transaction: Transaction) -> bool {
    let notes = household_notes(cluster);
    cluster.write(device, transaction.set(notes, "ruled", "yes"));
    cluster.settle();

    read_committed(cluster, NAS, "ruled").is_some()
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

#[test]
fn a_row_that_breaks_a_rule_isnt_inserted() {
    let (mut cluster, table) = ruled();

    insert(&mut cluster, LAPTOP, table, "grace", -1);
    insert(&mut cluster, LAPTOP, table, "katherine", 101);
    cluster.settle();

    everywhere(&cluster, table, &[("ada", 36), ("katherine", 101)]);
}

#[test]
fn a_row_cant_be_changed_into_one_that_breaks_a_rule() {
    let (mut cluster, table) = ruled();
    let ada = row(&cluster, LAPTOP, table, "ada").expect("inserted");

    let younger = Transaction::new().update(table, &ada, [(age(table), (-5).into())]);
    assert!(!happened(&mut cluster, LAPTOP, younger));
    let cleared = Transaction::new().update(table, &ada, [(age(table), Value::Null)]);
    assert!(!happened(&mut cluster, LAPTOP, cleared));
    let older = Transaction::new().update(table, &ada, [(age(table), 37.into())]);
    assert!(happened(&mut cluster, LAPTOP, older));

    everywhere(&cluster, table, &[("ada", 37)]);
}

#[test]
fn a_change_to_another_column_is_held_to_the_rule_too() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();
    cluster.disconnect_all();

    // The rule and a change that breaks it are written apart. The
    // phone writes more first, so its change is placed last.
    cluster.write(LAPTOP, Transaction::new().add_rule(table, born(table)));
    write(&mut cluster, PHONE, "busy", "yes");
    let ada = row(&cluster, PHONE, table, "ada").expect("inserted");
    cluster.write(PHONE, {
        let changes = [(age(table), (-1).into()), (name(table), "Ada".into())];

        Transaction::new().update(table, &ada, changes)
    });
    cluster.connect_all();
    cluster.settle();

    everywhere(&cluster, table, &[("ada", 36)]);
    let ada = row(&cluster, NAS, table, "ada").expect("inserted");
    assert_ne!(ada.get(name(table)), &Value::from("Ada"));
}

#[test]
fn a_rule_the_rows_already_break_isnt_made() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", -36);
    cluster.settle();

    let rule = Transaction::new().add_rule(table, born(table));
    assert!(!happened(&mut cluster, PHONE, rule));

    // With nothing in force, rows like it are still fine.
    insert(&mut cluster, LAPTOP, table, "grace", -85);
    cluster.settle();
    everywhere(&cluster, table, &[("ada", -36), ("grace", -85)]);
}

#[test]
fn rows_can_be_mended_and_ruled_in_one_go() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    insert(&mut cluster, PHONE, table, "ada", -36);
    cluster.settle();

    let mended = Transaction::new()
        .update_where(
            table,
            Filter::all().below(age(table), 0),
            [(age(table), 0.into())],
        )
        .add_rule(table, born(table));
    assert!(happened(&mut cluster, PHONE, mended));

    insert(&mut cluster, LAPTOP, table, "grace", -85);
    cluster.settle();
    everywhere(&cluster, table, &[("ada", 0)]);
}

#[test]
fn a_change_that_waited_is_held_to_the_rule() {
    let (mut cluster, table) = ruled();

    let swept = Transaction::new().update_where(table, Filter::all(), [(age(table), (-1).into())]);

    assert!(!happened(&mut cluster, PHONE, swept));
    everywhere(&cluster, table, &[("ada", 36)]);
}

#[test]
fn a_rule_is_pending_until_its_committed_and_binds_nothing_before() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    cluster.settle();
    cluster.disconnect_all();

    cluster.write(PHONE, Transaction::new().add_rule(table, born(table)));
    insert(&mut cluster, PHONE, table, "grace", -1);

    let pending = block_on(cluster.devices[PHONE].pending(cluster.home)).expect("read");
    let rule = Waiting::Rule {
        filter: born(table),
    };
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].change, rule);

    // It's shown until the rule that turns it down is committed.
    assert_eq!(ages(&cluster, PHONE, table).len(), 1);
    assert_eq!(rules(&cluster, PHONE, table), []);
    cluster.connect_all();
    cluster.settle();
    everywhere(&cluster, table, &[]);
}

/// The rules a table has in force, as the device reads them.
fn rules(cluster: &Sim, device: usize, table: ModelId) -> Vec<Filter> {
    let results = cluster.read(device, &Query::table(cluster.home, table));
    let table = Table::read(&results).expect("created");

    table.rules().to_vec()
}

/// An application checks a row against the rules before it writes
/// one, and nothing else says what they are once they're committed.
#[test]
fn a_table_says_which_rules_are_in_force() {
    let (mut cluster, table) = ruled();

    for device in [PHONE, NAS] {
        assert_eq!(rules(&cluster, device, table), [born(table)]);
    }

    cluster.write(PHONE, Transaction::new().lift_rule(table, &born(table)));

    assert_eq!(rules(&cluster, PHONE, table), []);
}

#[test]
fn a_lifted_rule_binds_nothing() {
    let (mut cluster, table) = ruled();

    cluster.write(LAPTOP, Transaction::new().lift_rule(table, &born(table)));
    insert(&mut cluster, LAPTOP, table, "grace", -1);
    cluster.settle();

    everywhere(&cluster, table, &[("ada", 36), ("grace", -1)]);
    let again = Transaction::new().lift_rule(table, &born(table));
    assert!(!happened(&mut cluster, PHONE, again));
}

#[test]
fn a_row_can_break_a_rule_and_be_gone_by_the_end() {
    let (mut cluster, table) = ruled();
    let ada = row(&cluster, PHONE, table, "ada").expect("inserted");

    let leaving = Transaction::new()
        .update(table, &ada, [(age(table), (-1).into())])
        .delete_row(table, &ada);

    assert!(happened(&mut cluster, PHONE, leaving));
    everywhere(&cluster, table, &[]);
}

#[test]
fn rules_come_with_a_snapshot() {
    let (mut cluster, table) = ruled();
    cluster.isolate(FRIEND);
    insert(&mut cluster, PHONE, table, "katherine", 101);
    cluster.settle();
    for device in [PHONE, LAPTOP, NAS] {
        cluster.compact(device, cluster.home);
    }

    cluster.connect_all();
    cluster.settle();
    insert(&mut cluster, LAPTOP, table, "grace", -1);
    cluster.settle();

    everywhere(&cluster, table, &[("ada", 36), ("katherine", 101)]);
}
