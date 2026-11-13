//! Unique columns: a value goes to one row, and comes back when the
//! row lets go of it.

use super::*;
use crate::model::table::Place;

pub(in crate::node::tests) fn accounts(cluster: &Cluster) -> ModelId {
    ModelId::table(genesis(cluster).id(), [9; 32])
}

pub(in crate::node::tests) fn email(table: ModelId) -> ColumnId {
    ColumnId::text(table, [1; 32]).unique()
}

fn plan(table: ModelId) -> ColumnId {
    ColumnId::text(table, [2; 32])
}

pub(in crate::node::tests) fn open(cluster: &mut Cluster, device: usize) -> ModelId {
    let (agreement, table) = (genesis(cluster).id(), accounts(cluster));
    cluster.run(device, |batch| {
        let transaction = batch.transaction(agreement);
        let columns = [(email(table), "email"), (plan(table), "plan")];

        transaction.create_table(table, columns).commit()
    });

    table
}

fn create_accounts(cluster: &mut Cluster, device: usize) -> ModelId {
    let table = open(cluster, device);
    cluster.settle();
    table
}

/// What the table's storage says about who has which address, on a
/// device: first going by the rows, then by the keys that take the
/// addresses. They should say the same thing.
pub(in crate::node::tests) fn taken(
    cluster: &Cluster,
    device: usize,
) -> [Vec<(Vec<u8>, Vec<u8>)>; 2] {
    let table = accounts(cluster);
    let query = Query::table(genesis(cluster).id(), table);
    let store = &cluster.devices[device].store;
    let prefixes = query.storage_prefixes();
    let scanned: Vec<_> = prefixes.flat_map(|prefix| store.scan(prefix)).collect();
    let results = query.results(&scanned);

    let read = Table::read(&results);
    let rows = read.iter().flat_map(Table::rows);
    let addressed = rows.filter(|row| row.get(email(table)) != &Value::Null);
    let by_row = addressed.map(|row| {
        let place = Place::unique(email(table), row.get(email(table)));

        (place, row.key().to_vec())
    });

    let mut by_row: Vec<_> = by_row.collect();
    by_row.sort();
    let unique =
        |(key, _): &(Vec<u8>, Vec<u8>)| matches!(Place::parse(key), Some(Place::Unique(..)));
    let by_key = results.iter().filter(|it| unique(it)).cloned().collect();

    [by_row, by_key]
}

pub(in crate::node::tests) fn sign_up(
    cluster: &mut Cluster,
    device: usize,
    key: &str,
    address: Option<&str>,
) {
    let (agreement, table) = (genesis(cluster).id(), accounts(cluster));
    let address = address.map_or(Value::Null, Value::from);
    let cells = [(email(table), address), (plan(table), "free".into())];
    let key = key.to_owned();
    cluster.run(device, |batch| {
        batch
            .transaction(agreement)
            .insert(table, key, cells)
            .commit()
    });
}

pub(in crate::node::tests) fn account(cluster: &Cluster, device: usize, key: &str) -> Option<Row> {
    let table = read_table(cluster, device, accounts(cluster))?;

    table.row(key.as_bytes()).cloned()
}

pub(in crate::node::tests) fn update(
    cluster: &mut Cluster,
    device: usize,
    row: &Row,
    column: ColumnId,
    value: &str,
) {
    let (agreement, table) = (genesis(cluster).id(), accounts(cluster));
    let changes = [(column, Value::from(value))];
    cluster.run(device, |batch| {
        batch
            .transaction(agreement)
            .update(table, row, changes)
            .commit()
    });
}

pub(in crate::node::tests) fn close(cluster: &mut Cluster, device: usize, row: &Row) {
    let (agreement, table) = (genesis(cluster).id(), accounts(cluster));
    cluster.run(device, |batch| {
        batch.transaction(agreement).delete_row(table, row).commit()
    });
}

/// Who has an address, on a device: the keys of every row that does.
fn holders(cluster: &Cluster, device: usize, address: &str) -> Vec<String> {
    let table = accounts(cluster);
    let read = read_table(cluster, device, table);
    let rows = read.iter().flat_map(Table::rows);
    let has = |row: &&Row| row.get(email(table)) == &Value::from(address);

    rows.filter(has)
        .map(|row| String::from_utf8_lossy(row.key()).into_owned())
        .collect()
}

#[test]
fn a_unique_column_is_a_different_column() {
    let table = ModelId::table(AgreementId::from_bytes([0; 32]), [9; 32]);

    assert_ne!(email(table), ColumnId::text(table, [1; 32]));
    assert_eq!(email(table), email(table).unique());
}

/// Two devices each sign someone up with the same address while apart,
/// under different keys. Only one of the rows can exist.
#[test]
fn one_row_gets_a_unique_value() {
    let mut cluster = household();
    create_accounts(&mut cluster, PHONE);
    cluster.disconnect_everyone();
    cluster.settle();

    sign_up(&mut cluster, PHONE, "ada", Some("a@example.com"));
    sign_up(&mut cluster, LAPTOP, "augusta", Some("a@example.com"));
    cluster.settle();
    assert_eq!(holders(&cluster, PHONE, "a@example.com"), ["ada"]);
    assert_eq!(holders(&cluster, LAPTOP, "a@example.com"), ["augusta"]);

    cluster.connect_everyone();
    cluster.settle();

    let winner = holders(&cluster, PHONE, "a@example.com");
    assert_eq!(winner.len(), 1);
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(holders(&cluster, device, "a@example.com"), winner);
        let rows = read_table(&cluster, device, accounts(&cluster)).map(|it| it.rows().len());
        assert_eq!(rows, Some(1), "device {device}");
        assert_eq!(stored_frontier(&cluster, device), [], "device {device}");
    }
}

#[test]
fn a_settled_unique_value_cant_be_taken() {
    let mut cluster = household();
    create_accounts(&mut cluster, PHONE);
    sign_up(&mut cluster, PHONE, "ada", Some("a@example.com"));
    cluster.settle();
    cluster.restart(LAPTOP);
    cluster.connect_everyone();

    sign_up(&mut cluster, LAPTOP, "augusta", Some("a@example.com"));
    sign_up(&mut cluster, LAPTOP, "grace", Some("g@example.com"));
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(holders(&cluster, device, "a@example.com"), ["ada"]);
        assert_eq!(holders(&cluster, device, "g@example.com"), ["grace"]);
        assert_eq!(stored_log_len(&cluster, device), 0, "device {device}");
    }
}

#[test]
fn any_number_of_rows_can_have_no_value() {
    let mut cluster = household();
    create_accounts(&mut cluster, PHONE);
    sign_up(&mut cluster, PHONE, "ada", None);
    sign_up(&mut cluster, LAPTOP, "grace", None);
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert!(account(&cluster, device, "ada").is_some());
        assert!(account(&cluster, device, "grace").is_some());
    }
}

#[test]
fn an_update_changes_some_columns_and_keeps_the_rest() {
    let mut cluster = household();
    let table = create_accounts(&mut cluster, PHONE);
    sign_up(&mut cluster, PHONE, "ada", Some("a@example.com"));
    cluster.settle();

    let ada = account(&cluster, LAPTOP, "ada").expect("signed up");
    update(&mut cluster, LAPTOP, &ada, plan(table), "paid");
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let ada = account(&cluster, device, "ada").expect("signed up");
        assert_eq!(ada.get(plan(table)), &Value::from("paid"));
        assert_eq!(ada.get(email(table)), &Value::from("a@example.com"));
    }

    // It still has its address, and nobody else can.
    sign_up(&mut cluster, NAS, "augusta", Some("a@example.com"));
    cluster.settle();
    assert_eq!(holders(&cluster, PHONE, "a@example.com"), ["ada"]);
}

/// A row that changes its address gives the old one back, and takes
/// the new one like a new row would.
#[test]
fn changing_a_unique_value_frees_the_old_one() {
    let mut cluster = household();
    let table = create_accounts(&mut cluster, PHONE);
    sign_up(&mut cluster, PHONE, "ada", Some("a@example.com"));
    sign_up(&mut cluster, PHONE, "grace", Some("g@example.com"));
    cluster.settle();

    // Grace can't have Ada's.
    let grace = account(&cluster, LAPTOP, "grace").expect("signed up");
    update(&mut cluster, LAPTOP, &grace, email(table), "a@example.com");
    cluster.settle();
    assert_eq!(holders(&cluster, NAS, "a@example.com"), ["ada"]);
    assert_eq!(holders(&cluster, NAS, "g@example.com"), ["grace"]);

    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    update(&mut cluster, PHONE, &ada, email(table), "ada@example.com");
    cluster.settle();
    let grace = account(&cluster, LAPTOP, "grace").expect("signed up");
    update(&mut cluster, LAPTOP, &grace, email(table), "a@example.com");
    sign_up(&mut cluster, NAS, "augusta", Some("g@example.com"));
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(holders(&cluster, device, "ada@example.com"), ["ada"]);
        assert_eq!(holders(&cluster, device, "a@example.com"), ["grace"]);
        assert_eq!(holders(&cluster, device, "g@example.com"), ["augusta"]);
        assert_eq!(stored_log_len(&cluster, device), 0, "device {device}");
    }
}

#[test]
fn deleting_a_row_frees_its_unique_values() {
    let mut cluster = household();
    create_accounts(&mut cluster, PHONE);
    sign_up(&mut cluster, PHONE, "ada", Some("a@example.com"));
    cluster.settle();

    let ada = account(&cluster, LAPTOP, "ada").expect("signed up");
    close(&mut cluster, LAPTOP, &ada);
    sign_up(&mut cluster, LAPTOP, "augusta", Some("a@example.com"));
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(account(&cluster, device, "ada"), None);
        assert_eq!(holders(&cluster, device, "a@example.com"), ["augusta"]);
    }
}

/// Two devices read a row while apart. One changes its address, the
/// other deletes it. Whichever is second was working from a row that's
/// no longer there, and didn't happen: an address is never given back
/// twice, or left taken by a row that's gone.
#[test]
fn a_change_to_a_row_thats_since_changed_never_happens() {
    let mut cluster = household();
    let table = create_accounts(&mut cluster, PHONE);
    sign_up(&mut cluster, PHONE, "ada", Some("a@example.com"));
    cluster.settle();
    cluster.disconnect_everyone();
    cluster.settle();

    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    update(&mut cluster, PHONE, &ada, email(table), "ada@example.com");
    close(&mut cluster, LAPTOP, &ada);
    cluster.connect_everyone();
    cluster.settle();

    // Whatever happened, both addresses are as free as they look.
    let renamed = account(&cluster, NAS, "ada").is_some();
    sign_up(&mut cluster, NAS, "one", Some("a@example.com"));
    sign_up(&mut cluster, NAS, "two", Some("ada@example.com"));
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(account(&cluster, device, "ada").is_some(), renamed);
        assert_eq!(holders(&cluster, device, "a@example.com"), ["one"]);
        let new = if renamed { "ada" } else { "two" };
        assert_eq!(holders(&cluster, device, "ada@example.com"), [new]);
        assert_eq!(stored_frontier(&cluster, device), [], "device {device}");
    }
}

/// A write that doesn't say what the row was can't give its values
/// back, so a table with a unique column doesn't take it.
#[test]
fn a_table_with_a_unique_column_only_takes_changes_to_rows_that_were_read() {
    let mut cluster = household();
    let (agreement, table) = (genesis(&cluster).id(), create_accounts(&mut cluster, PHONE));
    sign_up(&mut cluster, PHONE, "ada", Some("a@example.com"));
    cluster.settle();

    let cells = || [(email(table), Value::from("other@example.com"))];
    cluster.run(LAPTOP, |batch| {
        let transaction = batch.transaction(agreement);

        transaction.replace(table, "ada", cells()).commit()
    });
    cluster.run(LAPTOP, |batch| {
        let transaction = batch.transaction(agreement);

        transaction.upsert(table, "ada", cells()).commit()
    });
    cluster.run(LAPTOP, |batch| {
        batch.transaction(agreement).delete(table, "ada").commit()
    });
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(holders(&cluster, device, "a@example.com"), ["ada"]);
        assert_eq!(holders(&cluster, device, "other@example.com"), [""; 0]);
        assert_eq!(stored_log_len(&cluster, device), 0, "device {device}");
    }
}

#[test]
fn a_row_is_found_by_its_unique_value() {
    let mut cluster = household();
    let table = create_accounts(&mut cluster, PHONE);
    sign_up(&mut cluster, PHONE, "ada", Some("a@example.com"));
    cluster.settle();

    let read = read_table(&cluster, NAS, table).expect("created");
    let found = read.find(email(table), &"a@example.com".into());
    assert_eq!(found.map(Row::key), Some(&b"ada"[..]));
    assert_eq!(read.find(email(table), &"b@example.com".into()), None);
}
