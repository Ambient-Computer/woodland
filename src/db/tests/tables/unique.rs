//! Unique columns: a value goes to one row, and comes back when the
//! row lets go of it.

use super::*;
use crate::model::table::Place;

pub(in crate::db::tests) fn accounts(cluster: &Sim) -> ModelId {
    ModelId::table(cluster.home, [9; 32])
}

pub(in crate::db::tests) fn email(table: ModelId) -> ColumnId {
    ColumnId::text(table, [1; 32]).unique()
}

pub(super) fn plan(table: ModelId) -> ColumnId {
    ColumnId::text(table, [2; 32])
}

/// How much an account pays for. Accounts can be found by it.
pub(in crate::db::tests) fn tier(table: ModelId) -> ColumnId {
    ColumnId::integer(table, [3; 32]).indexed()
}

pub(in crate::db::tests) fn open(cluster: &mut Sim, device: usize) -> ModelId {
    let table = accounts(cluster);
    cluster.write(device, {
        let transaction = Transaction::new();
        let columns = [
            (email(table), "email"),
            (plan(table), "plan"),
            (tier(table), "tier"),
        ];

        transaction.create_table(table, columns)
    });

    table
}

pub(super) fn create_accounts(cluster: &mut Sim, device: usize) -> ModelId {
    let table = open(cluster, device);
    cluster.settle();
    table
}

/// What the table's storage says about who has which address and
/// tier, on a device: first going by the rows, then by the keys they're
/// looked up under. They should say the same thing.
pub(in crate::db::tests) fn taken(cluster: &Sim, device: usize) -> [Vec<(Vec<u8>, Vec<u8>)>; 2] {
    let table = accounts(cluster);
    let query = Query::table(cluster.home, table);
    let store = cluster.reader(device);
    let results = store.read(&query);

    let read = Table::read(&results);
    let rows = read.iter().flat_map(Table::rows);
    let by_row = rows.flat_map(|row| {
        let (address, paid) = (row.get(email(table)), row.get(tier(table)));
        let address = (address != &Value::Null)
            .then(|| (Place::unique(email(table), address), row.key().to_vec()));
        let paid = (paid != &Value::Null)
            .then(|| (Place::index(tier(table), paid, row.key()), Vec::new()));

        address.into_iter().chain(paid)
    });

    let mut by_row: Vec<_> = by_row.collect();
    by_row.sort();
    let unique = |(key, _): &(Vec<u8>, Vec<u8>)| {
        matches!(
            Place::parse(key),
            Some(Place::Unique(..) | Place::Index(..))
        )
    };
    let by_key = results.iter().filter(|it| unique(it)).cloned().collect();

    [by_row, by_key]
}

pub(in crate::db::tests) fn sign_up(
    cluster: &mut Sim,
    device: usize,
    key: &str,
    address: Option<&str>,
) {
    let table = accounts(cluster);
    let address = address.map_or(Value::Null, Value::from);
    let cells = [(email(table), address), (plan(table), "free".into())];
    let key = key.to_owned();
    cluster.write(device, Transaction::new().insert(table, key, cells));
}

pub(in crate::db::tests) fn account(cluster: &Sim, device: usize, key: &str) -> Option<Row> {
    let table = read_table(cluster, device, accounts(cluster))?;

    table.row(key.as_bytes()).cloned()
}

pub(in crate::db::tests) fn update(
    cluster: &mut Sim,
    device: usize,
    row: &Row,
    column: ColumnId,
    value: impl Into<Value>,
) {
    let table = accounts(cluster);
    let changes = [(column, value.into())];
    cluster.write(device, Transaction::new().update(table, row, changes));
}

pub(in crate::db::tests) fn close(cluster: &mut Sim, device: usize, row: &Row) {
    let table = accounts(cluster);
    cluster.write(device, Transaction::new().delete_row(table, row));
}

/// Who has an address, on a device: the keys of every row that does.
pub(super) fn holders(cluster: &Sim, device: usize, address: &str) -> Vec<String> {
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
    cluster.disconnect_all();
    cluster.settle();

    sign_up(&mut cluster, PHONE, "ada", Some("a@example.com"));
    sign_up(&mut cluster, LAPTOP, "augusta", Some("a@example.com"));
    cluster.settle();
    assert_eq!(holders(&cluster, PHONE, "a@example.com"), ["ada"]);
    assert_eq!(holders(&cluster, LAPTOP, "a@example.com"), ["augusta"]);

    cluster.connect_all();
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
    cluster.connect_all();

    sign_up(&mut cluster, LAPTOP, "augusta", Some("a@example.com"));
    sign_up(&mut cluster, LAPTOP, "grace", Some("g@example.com"));
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(holders(&cluster, device, "a@example.com"), ["ada"]);
        assert_eq!(holders(&cluster, device, "g@example.com"), ["grace"]);
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
    cluster.disconnect_all();
    cluster.settle();

    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    update(&mut cluster, PHONE, &ada, email(table), "ada@example.com");
    close(&mut cluster, LAPTOP, &ada);
    cluster.connect_all();
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

/// A change to a unique column doesn't get in the way of a change to
/// another column.
#[test]
fn changes_to_a_unique_column_and_another_both_happen() {
    let mut cluster = household();
    let table = create_accounts(&mut cluster, PHONE);
    sign_up(&mut cluster, PHONE, "ada", Some("a@example.com"));
    cluster.settle();
    cluster.disconnect_all();
    cluster.settle();

    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    update(&mut cluster, PHONE, &ada, email(table), "ada@example.com");
    update(&mut cluster, LAPTOP, &ada, plan(table), "paid");
    cluster.connect_all();
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let ada = account(&cluster, device, "ada").expect("signed up");
        assert_eq!(ada.get(plan(table)), &Value::from("paid"));
        assert_eq!(ada.get(email(table)), &Value::from("ada@example.com"));
        assert_eq!(stored_frontier(&cluster, device), [], "device {device}");
    }
}

/// Two devices each give a row a new address while apart. Only one
/// does: the other was working from addresses the row no longer has.
#[test]
fn only_one_of_two_changes_to_a_rows_unique_columns_happens() {
    let mut cluster = household();
    let table = create_accounts(&mut cluster, PHONE);
    sign_up(&mut cluster, PHONE, "ada", Some("a@example.com"));
    cluster.settle();
    cluster.disconnect_all();
    cluster.settle();

    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    update(&mut cluster, PHONE, &ada, email(table), "one@example.com");
    update(&mut cluster, LAPTOP, &ada, email(table), "two@example.com");
    cluster.connect_all();
    cluster.settle();

    let [by_row, by_key] = taken(&cluster, NAS);
    assert_eq!(by_row.len(), 1);
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(taken(&cluster, device), [by_row.clone(), by_key.clone()]);
        assert_eq!(holders(&cluster, device, "a@example.com"), [""; 0]);
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
