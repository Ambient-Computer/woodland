//! Rows that depend on rows in another table, and deleting what's
//! depended on.

use super::unique::{account, accounts, create_accounts, sign_up};
use super::*;

pub(in crate::db::tests) fn orders(cluster: &Sim) -> ModelId {
    ModelId::table(cluster.home, [8; 32])
}

/// Whose order it is: the key of an account.
pub(in crate::db::tests) fn buyer(table: ModelId) -> ColumnId {
    ColumnId::text(table, [1; 32])
}

/// Accounts and orders, and Ada's account, known to everyone.
pub(super) fn shop() -> (Sim, GroupId) {
    let mut cluster = household();
    let group = cluster.home;
    create_accounts(&mut cluster, PHONE);
    open_orders(&mut cluster, PHONE);
    sign_up(&mut cluster, PHONE, "ada", None);
    cluster.settle();
    let orders = orders(&cluster);
    cluster.keep_indexes(orders, buyer(orders));

    (cluster, group)
}

pub(in crate::db::tests) fn open_orders(cluster: &mut Sim, device: usize) {
    let table = orders(cluster);
    cluster.write(device, {
        let transaction = Transaction::new();

        transaction.create_table(table, [(buyer(table), "buyer")])
    });
}

/// Places an order for an account, as the device read it. The order
/// refers to the account.
pub(in crate::db::tests) fn order(cluster: &mut Sim, device: usize, key: &str, account: &Row) {
    let (accounts, orders) = (accounts(cluster), orders(cluster));
    let name = String::from_utf8_lossy(account.key()).into_owned();
    let cells = [(buyer(orders), Value::from(name))];
    let key = key.to_owned();
    cluster.write(device, {
        let transaction = Transaction::new();

        transaction
            .insert(orders, key, cells)
            .refer(accounts, account)
    });
}

/// An account's orders, as the device reads them: found through the
/// index, then read one at a time.
pub(in crate::db::tests) fn orders_of(
    cluster: &Sim,
    device: usize,
    buyer_key: &str,
    committed: bool,
) -> Vec<Row> {
    let (group, table) = (cluster.home, orders(cluster));
    let store = cluster.reader(device);
    let committed = |query: Query| if committed { query.committed() } else { query };
    let found = committed(Query::lookup(group, table, buyer(table), &buyer_key.into()));
    let row = |key: Vec<u8>| {
        let read = Table::read(&store.read(&committed(Query::row(group, table, &key))))?;

        read.row(&key).cloned()
    };

    let keys = Table::row_keys(&store.read(&found));

    keys.into_iter().filter_map(row).collect()
}

pub(in crate::db::tests) fn committed_account(
    cluster: &Sim,
    device: usize,
    key: &str,
) -> Option<Row> {
    let query = Query::row(cluster.home, accounts(cluster), key.as_bytes());
    let read = Table::read(&cluster.read(device, &query.committed()))?;

    read.row(key.as_bytes()).cloned()
}

pub(in crate::db::tests) fn retire(cluster: &mut Sim, device: usize, row: &Row) {
    let table = accounts(cluster);
    cluster.write(device, Transaction::new().retire(table, row));
}

/// Closes an account the way an application should, going only by
/// what is committed: not at all if it has orders and isn't retired,
/// and along with every order it has if it is.
pub(in crate::db::tests) fn close_with_orders(cluster: &mut Sim, device: usize, key: &str) {
    let Some(row) = committed_account(cluster, device, key) else {
        return;
    };

    if row.is_referred() && !row.is_retired() {
        return;
    }
    let (accounts, orders) = (accounts(cluster), orders(cluster));
    let placed = orders_of(cluster, device, key, true);
    cluster.write(device, {
        let transaction = Transaction::new();
        let emptied = placed.iter().fold(transaction, |transaction, order| {
            transaction.delete_row(orders, order)
        });

        emptied.delete_row(accounts, &row)
    });
}

fn close(cluster: &mut Sim, device: usize, row: &Row) {
    let table = accounts(cluster);
    cluster.write(device, Transaction::new().delete_row(table, row));
}

#[test]
fn a_row_can_refer_to_another() {
    let (mut cluster, _) = shop();
    let ada = account(&cluster, LAPTOP, "ada").expect("signed up");
    assert!(!ada.is_referred());

    order(&mut cluster, LAPTOP, "one", &ada);
    order(&mut cluster, NAS, "two", &ada);
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(orders_of(&cluster, device, "ada", false).len(), 2);
        let ada = account(&cluster, device, "ada").expect("signed up");
        assert!(ada.is_referred() && !ada.is_retired(), "device {device}");
    }
}

#[test]
fn nothing_refers_to_a_row_thats_gone() {
    let (mut cluster, _) = shop();
    let ada = account(&cluster, LAPTOP, "ada").expect("signed up");
    close(&mut cluster, PHONE, &ada);
    cluster.settle();

    order(&mut cluster, LAPTOP, "one", &ada);
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(account(&cluster, device, "ada"), None);
        assert_eq!(orders_of(&cluster, device, "ada", false), []);
    }
}

/// The phone closes an account while the laptop, apart from it, places
/// an order for it. Only one of those happens, whichever was first:
/// there's never an order for an account that isn't there.
#[test]
fn a_reference_and_a_delete_written_apart_dont_both_happen() {
    let (mut cluster, _) = shop();
    cluster.disconnect_all();
    cluster.settle();

    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    close(&mut cluster, PHONE, &ada);
    order(&mut cluster, LAPTOP, "one", &ada);
    cluster.settle();
    assert_eq!(account(&cluster, PHONE, "ada"), None);
    assert_eq!(orders_of(&cluster, LAPTOP, "ada", false).len(), 1);

    cluster.connect_all();
    cluster.settle();

    let ordered = orders_of(&cluster, PHONE, "ada", false).len();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let exists = account(&cluster, device, "ada").is_some();
        assert_eq!(usize::from(exists), ordered, "device {device}");
        assert_eq!(orders_of(&cluster, device, "ada", false).len(), ordered);
        assert_eq!(stored_frontier(&cluster, device), [], "device {device}");
    }
}

#[test]
fn a_row_thats_been_referred_to_cant_just_be_deleted() {
    let (mut cluster, _) = shop();
    let ada = account(&cluster, LAPTOP, "ada").expect("signed up");
    order(&mut cluster, LAPTOP, "one", &ada);
    cluster.settle();

    // Knowing about the order doesn't help. Even with the order gone,
    // the account has to be retired first.
    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    let table = orders(&cluster);
    let placed = orders_of(&cluster, PHONE, "ada", false);
    cluster.write(PHONE, {
        let transaction = Transaction::new();

        transaction.delete_row(table, &placed[0])
    });
    close(&mut cluster, PHONE, &ada);
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert!(
            account(&cluster, device, "ada").is_some(),
            "device {device}"
        );
        assert_eq!(orders_of(&cluster, device, "ada", false), []);
    }
}

/// Deleting an account that has orders: retire it, wait for that to
/// be committed, then delete it and every order there is in one go. An
/// order placed by a device that hadn't heard lands after the
/// retirement, however early it was written, and doesn't happen.
#[test]
fn a_retired_row_is_deleted_with_everything_that_refers_to_it() {
    let (mut cluster, _) = shop();
    let ada = account(&cluster, LAPTOP, "ada").expect("signed up");
    order(&mut cluster, LAPTOP, "one", &ada);
    cluster.settle();
    cluster.disconnect(NAS, PHONE);
    cluster.disconnect(NAS, LAPTOP);
    cluster.disconnect(NAS, FRIEND);
    cluster.settle();

    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    order(&mut cluster, NAS, "two", &ada);
    retire(&mut cluster, PHONE, &ada);
    cluster.settle();

    // The phone and the laptop commit the retirement without the NAS.
    // Nothing the NAS wrote in the meantime can land before it.
    let ada = committed_account(&cluster, PHONE, "ada").expect("signed up");
    assert!(ada.is_retired());

    cluster.connect_all();
    cluster.settle();
    for device in [PHONE, NAS] {
        let placed = orders_of(&cluster, device, "ada", true);
        assert!(placed.iter().all(|order| order.key() != b"two"));
    }

    // No more orders, from anyone.
    order(&mut cluster, LAPTOP, "three", &ada);
    cluster.settle();

    let placed = orders_of(&cluster, PHONE, "ada", true);
    assert!(placed.iter().all(|order| order.key() != b"three"));
    let orders = orders(&cluster);
    close_with_orders(&mut cluster, PHONE, "ada");
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(account(&cluster, device, "ada"), None, "device {device}");
        let left = read_table(&cluster, device, orders).map(|table| table.rows().len());
        assert_eq!(left, Some(0), "device {device}");
    }

    // The key is free, and whoever takes it starts over.
    sign_up(&mut cluster, LAPTOP, "ada", None);
    cluster.settle();
    let ada = account(&cluster, NAS, "ada").expect("signed up again");
    assert!(!ada.is_referred() && !ada.is_retired());
}

/// A retired row can be restored. A delete written while it was
/// retired doesn't happen once it's been restored, even if it's
/// retired again: what refers to it may have changed.
#[test]
fn a_delete_goes_with_the_retirement_it_was_written_in() {
    let (mut cluster, _) = shop();
    let table = accounts(&cluster);
    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    retire(&mut cluster, PHONE, &ada);
    cluster.settle();
    cluster.disconnect_all();
    cluster.settle();

    let retired = account(&cluster, PHONE, "ada").expect("signed up");
    cluster.write(LAPTOP, Transaction::new().restore(table, &retired));
    cluster.settle();
    let restored = account(&cluster, LAPTOP, "ada").expect("signed up");
    assert!(!restored.is_retired());
    order(&mut cluster, LAPTOP, "one", &restored);
    retire(&mut cluster, LAPTOP, &restored);
    cluster.settle();

    // The phone's delete is written last, so it's last in the
    // timeline.
    for value in ["one", "two", "three", "four"] {
        write(&mut cluster, PHONE, "busy", value);
    }

    close(&mut cluster, PHONE, &retired);
    cluster.connect_all();
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let ada = account(&cluster, device, "ada").expect("still there");
        assert!(ada.is_retired(), "device {device}");
        assert_eq!(orders_of(&cluster, device, "ada", false).len(), 1);
        assert_eq!(stored_frontier(&cluster, device), [], "device {device}");
    }
}
