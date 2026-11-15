//! Reading some of a table: a range of rows, or the rows that hold a
//! value in an indexed column.

use super::unique::{
    account, accounts, create_accounts, email, plan, sign_up, taken, tier, update,
};
use std::ops::Bound;

use super::*;

/// Signs someone up with no address, at a tier.
fn sign_up_at(cluster: &mut Sim, device: usize, key: &str, paid: i64) {
    let table = accounts(cluster);
    let cells = [(tier(table), Value::from(paid))];
    let key = key.to_owned();
    cluster.write(device, Transaction::new().insert(table, key, cells));
}

/// The keys of the rows a query for part of the table reads.
fn keys_read(cluster: &Sim, device: usize, query: &Query) -> Vec<String> {
    let read = Table::read(&cluster.read(device, query));
    let rows = read.iter().flat_map(Table::rows);

    rows.map(|row| String::from_utf8_lossy(row.key()).into_owned())
        .collect()
}

/// The keys of the rows an index query finds.
fn found(cluster: &Sim, device: usize, query: &Query) -> Vec<String> {
    let keys = Table::row_keys(&cluster.read(device, query));

    keys.iter()
        .map(|key| String::from_utf8_lossy(key).into_owned())
        .collect()
}

fn signed_up() -> (Sim, GroupId, ModelId) {
    let mut cluster = household();
    let table = create_accounts(&mut cluster, PHONE);
    for (key, paid) in [("ada", 1), ("al", 9), ("alan", 3), ("grace", 2)] {
        sign_up_at(&mut cluster, PHONE, key, paid);
    }

    cluster.settle();
    let group = cluster.home;

    (cluster, group, table)
}

#[test]
fn rows_are_read_a_range_at_a_time() {
    let (cluster, group, table) = signed_up();
    let read = |query: Query| keys_read(&cluster, NAS, &query);

    assert_eq!(read(Query::rows(group, table, "al".."b")), ["al", "alan"]);
    assert_eq!(read(Query::rows(group, table, "ada"..="al")), ["ada", "al"]);
    assert_eq!(read(Query::rows(group, table, .."al")), ["ada"]);
    assert_eq!(read(Query::rows(group, table, "alan"..)), ["alan", "grace"]);

    let after = (Bound::Excluded("al"), Bound::<&str>::Unbounded);
    assert_eq!(
        read(Query::rows::<&str>(group, table, after)),
        ["alan", "grace"]
    );
}

/// Reading one row reads its cells and the table's columns, and none
/// of a row whose key starts with its key.
#[test]
fn one_row_is_read_on_its_own() {
    let (cluster, group, table) = signed_up();
    let query = Query::row(group, table, b"al");
    let read = Table::read(&cluster.read(NAS, &query)).expect("created");

    assert_eq!(read.rows().len(), 1);
    assert_eq!(read.columns().count(), 3);
    let al = read.row(b"al").expect("signed up");
    assert_eq!(al.get(tier(table)), &Value::from(9));

    let nobody = Query::row(group, table, b"a");
    assert_eq!(keys_read(&cluster, NAS, &nobody), [""; 0]);
}

/// A row read on its own is as good as one read with the whole table:
/// it can be changed.
#[test]
fn a_row_read_on_its_own_can_be_changed() {
    let (mut cluster, group, table) = signed_up();
    let query = Query::row(group, table, b"grace");
    let read = Table::read(&cluster.read(LAPTOP, &query)).expect("created");
    let grace = read.row(b"grace").expect("signed up");

    update(&mut cluster, LAPTOP, grace, plan(table), "paid");
    cluster.settle();

    let grace = account(&cluster, NAS, "grace").expect("signed up");
    assert_eq!(grace.get(plan(table)), &Value::from("paid"));
}

#[test]
fn an_index_finds_rows_by_what_they_hold() {
    let (cluster, group, table) = signed_up();
    let find = |query: Query| found(&cluster, NAS, &query);
    let paid = tier(table);

    assert_eq!(
        find(Query::lookup(group, table, paid, &2.into())),
        ["grace"]
    );
    assert_eq!(find(Query::lookup(group, table, paid, &4.into())), [""; 0]);

    // In the order of the values, not of the rows.
    let from_two = Query::index(group, table, paid, Value::from(2)..);
    assert_eq!(find(from_two), ["grace", "alan", "al"]);
    let below_three = Query::index(group, table, paid, ..Value::from(3));
    assert_eq!(find(below_three), ["ada", "grace"]);
    let through_three = Query::index(group, table, paid, Value::from(2)..=Value::from(3));
    assert_eq!(find(through_three), ["grace", "alan"]);
    assert_eq!(find(Query::index(group, table, paid, ..)).len(), 4);
}

#[test]
fn an_index_sorts_numbers_by_size() {
    let mut cluster = household();
    let (group, table) = (cluster.home, create_accounts(&mut cluster, PHONE));
    for (key, paid) in [("a", 300), ("b", -1), ("c", 0), ("d", -300), ("e", 2)] {
        sign_up_at(&mut cluster, PHONE, key, paid);
    }

    cluster.settle();

    let everyone = Query::index(group, table, tier(table), ..);
    assert_eq!(
        found(&cluster, LAPTOP, &everyone),
        ["d", "b", "c", "e", "a"]
    );
    let negative = Query::index(group, table, tier(table), ..Value::from(0));
    assert_eq!(found(&cluster, LAPTOP, &negative), ["d", "b"]);
}

#[test]
fn an_index_follows_the_rows() {
    let (mut cluster, group, table) = signed_up();
    let at = |paid: i64| Query::lookup(group, table, tier(table), &paid.into());

    let ada = account(&cluster, LAPTOP, "ada").expect("signed up");
    update(&mut cluster, LAPTOP, &ada, tier(table), 2);
    delete(&mut cluster, LAPTOP, table, "grace");

    // The laptop sees it before anyone else has heard.
    cluster.disconnect_all();
    cluster.settle();
    assert_eq!(found(&cluster, LAPTOP, &at(2)), ["ada"]);
    assert_eq!(found(&cluster, LAPTOP, &at(2).committed()), ["grace"]);

    cluster.connect_all();
    cluster.settle();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(found(&cluster, device, &at(1)), [""; 0], "device {device}");
        assert_eq!(found(&cluster, device, &at(2)), ["ada"], "device {device}");
        assert_eq!(found(&cluster, device, &at(2).committed()), ["ada"]);
        let [by_row, by_key] = taken(&cluster, device);
        assert_eq!(by_row, by_key, "device {device}");
    }
}

#[test]
fn a_unique_column_is_an_index_too() {
    let mut cluster = household();
    let (group, table) = (cluster.home, create_accounts(&mut cluster, PHONE));
    sign_up(&mut cluster, PHONE, "ada", Some("a@example.com"));
    sign_up(&mut cluster, PHONE, "grace", Some("g@example.com"));
    cluster.settle();

    let address = Value::from("g@example.com");
    let query = Query::lookup(group, table, email(table), &address);
    assert_eq!(found(&cluster, NAS, &query), ["grace"]);
    let everyone = Query::index(group, table, email(table), ..);
    assert_eq!(found(&cluster, NAS, &everyone), ["ada", "grace"]);
}

#[test]
fn a_column_that_isnt_indexed_finds_nothing() {
    let (cluster, group, table) = signed_up();
    let query = Query::index(group, table, plan(table), ..);

    assert_eq!(query.clone().committed().storage_ranges(), []);
    assert_eq!(found(&cluster, NAS, &query), [""; 0]);
}

/// An index has to take back what a row held, so a change to an
/// indexed column goes by what was read, like a unique one. Two
/// devices that change it while apart can't both.
#[test]
fn only_one_of_two_changes_to_an_indexed_column_happens() {
    let (mut cluster, group, table) = signed_up();
    cluster.disconnect_all();
    cluster.settle();

    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    update(&mut cluster, PHONE, &ada, tier(table), 5);
    update(&mut cluster, LAPTOP, &ada, tier(table), 6);
    update(&mut cluster, NAS, &ada, plan(table), "paid");
    cluster.connect_all();
    cluster.settle();

    let at = |paid: i64| Query::lookup(group, table, tier(table), &paid.into());
    let (five, six) = (
        found(&cluster, PHONE, &at(5)),
        found(&cluster, PHONE, &at(6)),
    );
    assert_eq!(five.len() + six.len(), 1);
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(found(&cluster, device, &at(1)), [""; 0], "device {device}");
        assert_eq!(found(&cluster, device, &at(5)), five, "device {device}");
        let [by_row, by_key] = taken(&cluster, device);
        assert_eq!(by_row, by_key, "device {device}");

        // The change to a column that isn't indexed went ahead.
        let ada = account(&cluster, device, "ada").expect("signed up");
        assert_eq!(ada.get(plan(table)), &Value::from("paid"));
    }
}

#[test]
fn queries_for_part_of_a_table_know_what_changes_them() {
    let (mut cluster, group, table) = signed_up();
    let rows = Query::rows(group, table, "a".."b");
    let other_rows = Query::rows(group, table, "g"..);
    let paid_two = Query::lookup(group, table, tier(table), &2.into());
    let paid_seven = Query::lookup(group, table, tier(table), &7.into());

    let ada = account(&cluster, LAPTOP, "ada").expect("signed up");
    let before = cluster.changes[LAPTOP].len();
    update(&mut cluster, LAPTOP, &ada, tier(table), 7);
    cluster.settle();
    let changed = &cluster.changes[LAPTOP][before..];

    let affected = |query: &Query| changed.iter().any(|change| query.is_affected_by(change));
    assert!(affected(&rows));
    assert!(affected(&paid_seven));
    assert!(!affected(&other_rows));
    assert!(!affected(&paid_two));
}
