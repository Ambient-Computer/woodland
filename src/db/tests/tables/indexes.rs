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

    assert_eq!(found(&cluster, NAS, &query), [""; 0]);
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

/// The keys of the rows whose plan is `plan`, going by the index.
fn on_plan(cluster: &Sim, device: usize, name: &str, committed: bool) -> Vec<String> {
    let table = accounts(cluster);
    let query = Query::lookup(cluster.home, table, plan(table), &name.into());
    let query = if committed { query.committed() } else { query };

    found(cluster, device, &query)
}

fn set_plan(cluster: &mut Sim, device: usize, key: &str, name: &str) {
    let table = accounts(cluster);
    let row = account(cluster, device, key).expect("signed up");
    update(cluster, device, &row, plan(table), name);
}

/// An index is added to a table that already has rows, by one device,
/// for itself.
#[test]
fn an_index_is_a_devices_own_and_is_filled_from_the_rows() {
    let (mut cluster, group, table) = signed_up();
    for key in ["ada", "al", "alan"] {
        set_plan(&mut cluster, PHONE, key, "paid");
    }
    set_plan(&mut cluster, PHONE, "grace", "free");
    cluster.settle();
    assert_eq!(on_plan(&cluster, NAS, "paid", false), [""; 0]);
    assert!(!cluster.devices[NAS].has_index(group, table, plan(table)));

    // More rows than one step looks at.
    for row in 0..20 {
        sign_up_at(&mut cluster, PHONE, &format!("z{row:02}"), row);
        set_plan(&mut cluster, PHONE, &format!("z{row:02}"), "paid");
    }
    cluster.settle();
    cluster.wake(NAS);
    let outputs = block_on(cluster.devices[NAS].add_index(group, table, plan(table)));
    let outputs = outputs.expect("indexed");
    assert!(outputs.unfinished);
    assert!(outputs.sends.is_empty(), "nobody else hears of it");
    cluster.absorb(NAS, outputs);
    let mut steps = 0;
    while cluster.resume() {
        steps += 1;
        assert!(!cluster.devices[NAS].has_index(group, table, plan(table)) || steps > 3);
    }

    assert!(steps > 3, "a part at a time");
    assert!(cluster.devices[NAS].has_index(group, table, plan(table)));
    assert_eq!(on_plan(&cluster, NAS, "paid", false).len(), 23);
    assert_eq!(on_plan(&cluster, NAS, "paid", true).len(), 23);
    assert_eq!(on_plan(&cluster, NAS, "free", true), ["grace"]);
    assert_eq!(on_plan(&cluster, LAPTOP, "paid", false), [""; 0]);

    // It's there after a restart.
    cluster.restart(NAS);
    assert!(cluster.devices[NAS].has_index(group, table, plan(table)));
    assert_eq!(on_plan(&cluster, NAS, "free", true), ["grace"]);
}

/// Each level of read has the index its rows make.
#[test]
fn a_devices_index_follows_the_rows_at_both_levels() {
    let (mut cluster, _, table) = signed_up();
    for device in [PHONE, LAPTOP] {
        cluster.keep_index(device, table, plan(table));
    }
    set_plan(&mut cluster, PHONE, "ada", "paid");
    set_plan(&mut cluster, PHONE, "grace", "paid");
    cluster.settle();

    cluster.disconnect_all();
    set_plan(&mut cluster, LAPTOP, "ada", "free");
    delete(&mut cluster, LAPTOP, table, "grace");
    sign_up_at(&mut cluster, LAPTOP, "linus", 1);
    set_plan(&mut cluster, LAPTOP, "linus", "paid");
    cluster.settle();

    assert_eq!(on_plan(&cluster, LAPTOP, "paid", false), ["linus"]);
    assert_eq!(on_plan(&cluster, LAPTOP, "free", false), ["ada"]);
    assert_eq!(on_plan(&cluster, LAPTOP, "paid", true), ["ada", "grace"]);
    assert_eq!(on_plan(&cluster, LAPTOP, "free", true), [""; 0]);

    cluster.connect_all();
    cluster.settle();
    for device in [PHONE, LAPTOP] {
        for committed in [false, true] {
            assert_eq!(on_plan(&cluster, device, "paid", committed), ["linus"]);
            assert_eq!(on_plan(&cluster, device, "free", committed), ["ada"]);
        }
    }
}

/// No transaction writes an index, so an indexed column merges like
/// any other: two devices that change a row's other columns, or the
/// same one, are both taken.
#[test]
fn an_index_changes_nothing_about_how_a_column_is_written() {
    let (mut cluster, _, table) = signed_up();
    cluster.keep_index(PHONE, table, plan(table));
    cluster.disconnect_all();
    cluster.settle();

    set_plan(&mut cluster, PHONE, "ada", "paid");
    set_plan(&mut cluster, LAPTOP, "ada", "free");
    set_plan(&mut cluster, NAS, "al", "paid");
    cluster.connect_all();
    cluster.settle();

    let ada = account(&cluster, FRIEND, "ada").expect("signed up");
    let Value::Text(read) = ada.get(plan(table)).clone() else {
        panic!("one of them is read");
    };
    let other = if read == "paid" { "free" } else { "paid" };
    assert!(on_plan(&cluster, PHONE, &read, true).contains(&"ada".to_owned()));
    assert!(!on_plan(&cluster, PHONE, other, true).contains(&"ada".to_owned()));
    assert!(on_plan(&cluster, PHONE, "paid", true).contains(&"al".to_owned()));
}

/// A row that's deleted and put back under its key is another row,
/// and what the first held isn't in the index.
#[test]
fn an_index_tells_a_row_from_the_one_that_had_its_key() {
    let (mut cluster, _, table) = signed_up();
    cluster.keep_index(NAS, table, plan(table));
    set_plan(&mut cluster, PHONE, "ada", "paid");
    cluster.settle();
    delete(&mut cluster, PHONE, table, "ada");
    sign_up_at(&mut cluster, PHONE, "ada", 1);
    cluster.settle();

    assert_eq!(on_plan(&cluster, NAS, "paid", true), [""; 0]);
    assert_eq!(on_plan(&cluster, NAS, "paid", false), [""; 0]);
}

/// A device that catches up from a snapshot gets rows no entry told it
/// of. Its indexes start again.
#[test]
fn an_index_is_filled_again_after_a_snapshot() {
    let (mut cluster, group, table) = signed_up();
    cluster.keep_index(FRIEND, table, plan(table));
    cluster.isolate(FRIEND);
    for key in ["ada", "al", "alan", "grace"] {
        set_plan(&mut cluster, PHONE, key, "paid");
    }
    cluster.settle();
    for device in [PHONE, LAPTOP, NAS] {
        cluster.compact(device, group);
    }
    cluster.connect_all();
    cluster.settle();

    assert!(cluster.devices[FRIEND].has_index(group, table, plan(table)));
    assert_eq!(on_plan(&cluster, FRIEND, "paid", true).len(), 4);
}

#[test]
fn a_dropped_index_is_gone() {
    let (mut cluster, group, table) = signed_up();
    cluster.keep_index(NAS, table, plan(table));
    set_plan(&mut cluster, PHONE, "ada", "paid");
    cluster.settle();
    assert_eq!(on_plan(&cluster, NAS, "paid", true), ["ada"]);

    cluster.wake(NAS);
    let outputs = block_on(cluster.devices[NAS].drop_index(group, table, plan(table)));
    cluster.absorb(NAS, outputs.expect("dropped"));
    cluster.run();

    assert!(!cluster.devices[NAS].has_index(group, table, plan(table)));
    assert_eq!(on_plan(&cluster, NAS, "paid", true), [""; 0]);
    set_plan(&mut cluster, PHONE, "al", "paid");
    cluster.settle();
    assert_eq!(on_plan(&cluster, NAS, "paid", true), [""; 0]);
}

/// Rows nobody has committed are in the index an optimistic read uses,
/// when it's filled as well as afterwards.
#[test]
fn an_index_is_filled_from_rows_that_arent_committed_too() {
    let (mut cluster, group, table) = signed_up();
    cluster.isolate(LAPTOP);
    sign_up_at(&mut cluster, LAPTOP, "linus", 1);
    set_plan(&mut cluster, LAPTOP, "linus", "paid");
    cluster.settle();
    cluster.keep_index(LAPTOP, table, plan(table));

    assert!(cluster.devices[LAPTOP].has_index(group, table, plan(table)));
    assert_eq!(on_plan(&cluster, LAPTOP, "paid", false), ["linus"]);
    assert_eq!(on_plan(&cluster, LAPTOP, "paid", true), [""; 0]);
}

/// One device gives a row a plan while another deletes it, having read
/// it without one. Whichever comes first, the row is gone, and so is
/// its place in the index.
#[test]
fn a_row_deleted_from_under_a_change_leaves_the_index() {
    let (mut cluster, _, table) = signed_up();
    cluster.keep_index(NAS, table, plan(table));
    cluster.disconnect_all();
    cluster.settle();

    set_plan(&mut cluster, PHONE, "ada", "paid");
    delete(&mut cluster, LAPTOP, table, "ada");
    set_plan(&mut cluster, LAPTOP, "al", "paid");
    delete(&mut cluster, PHONE, table, "al");
    cluster.connect_all();
    cluster.settle();

    for committed in [false, true] {
        assert_eq!(on_plan(&cluster, NAS, "paid", committed), [""; 0]);
    }
}
