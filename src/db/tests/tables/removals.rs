//! Deleting a row that rows elsewhere refer to, in one go.

use super::references::{buyer, order, orders, shop};
use super::unique::{account, accounts, sign_up};
use super::*;

/// Where an account's orders are found.
fn placed(cluster: &Sim) -> [(ModelId, ColumnId); 1] {
    let orders = orders(cluster);

    [(orders, buyer(orders))]
}

fn order_keys(cluster: &Sim, device: usize) -> Vec<String> {
    let table = read_table(cluster, device, orders(cluster)).expect("created");
    let key = |row: &Row| String::from_utf8_lossy(row.key()).into_owned();

    table.rows().iter().map(key).collect()
}

/// Writes `transaction` along with a note, and says whether the note
/// was committed: whether the transaction happened.
fn happened(cluster: &mut Sim, device: usize, transaction: Transaction) -> bool {
    let notes = household_notes(cluster);
    cluster.write(device, transaction.set(notes, "removed", "yes"));
    cluster.settle();

    read_committed(cluster, NAS, "removed").is_some()
}

fn close(cluster: &Sim, row: &Row) -> Transaction {
    Transaction::new().delete_with(accounts(cluster), row, placed(cluster))
}

#[test]
fn a_row_goes_with_everything_that_refers_to_it() {
    let (mut cluster, _) = shop();
    sign_up(&mut cluster, PHONE, "grace", None);
    cluster.settle();
    let ada = account(&cluster, LAPTOP, "ada").expect("signed up");
    let grace = account(&cluster, LAPTOP, "grace").expect("signed up");
    order(&mut cluster, LAPTOP, "one", &ada);
    order(&mut cluster, LAPTOP, "two", &grace);
    cluster.settle();

    // An order the phone never hears of before it closes the account.
    cluster.isolate(NAS);
    order(&mut cluster, NAS, "three", &ada);
    let closing = close(&cluster, &ada);
    cluster.write(PHONE, closing);
    cluster.connect_all();
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(account(&cluster, device, "ada"), None, "device {device}");
        assert!(account(&cluster, device, "grace").is_some());
        assert_eq!(order_keys(&cluster, device), ["two"], "device {device}");
    }

    // Nothing of the old row is left to get in the way of a new one.
    sign_up(&mut cluster, LAPTOP, "ada", None);
    cluster.settle();
    let ada = account(&cluster, NAS, "ada").expect("signed up again");
    assert!(!ada.is_referred());
}

#[test]
fn nothing_goes_until_its_committed() {
    let (mut cluster, _) = shop();
    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    order(&mut cluster, PHONE, "one", &ada);
    cluster.settle();
    cluster.disconnect_all();

    let closing = close(&cluster, &ada);
    cluster.write(PHONE, closing);
    cluster.settle();

    assert!(account(&cluster, PHONE, "ada").is_some());
    assert_eq!(order_keys(&cluster, PHONE), ["one"]);
}

#[test]
fn a_row_that_took_another_ones_key_stays() {
    let (mut cluster, _) = shop();
    let table = accounts(&cluster);
    let first = account(&cluster, PHONE, "ada").expect("signed up");
    cluster.write(PHONE, Transaction::new().delete_row(table, &first));
    sign_up(&mut cluster, PHONE, "ada", None);
    cluster.settle();

    let closing = close(&cluster, &first);

    assert!(!happened(&mut cluster, LAPTOP, closing));
    assert!(account(&cluster, NAS, "ada").is_some());
}

#[test]
fn it_stops_at_a_row_something_else_refers_to() {
    let (mut cluster, _) = shop();
    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    order(&mut cluster, PHONE, "one", &ada);
    cluster.settle();
    let orders = orders(&cluster);
    let one = row(&cluster, PHONE, orders, "one").expect("placed");
    cluster.write(PHONE, Transaction::new().refer(orders, &one));
    cluster.settle();

    let closing = close(&cluster, &ada);

    assert!(!happened(&mut cluster, PHONE, closing));
    assert!(account(&cluster, NAS, "ada").is_some());
    assert_eq!(order_keys(&cluster, NAS), ["one"]);
}

#[test]
fn a_row_is_deleted_if_nothing_refers_to_it_any_more() {
    let (mut cluster, _) = shop();
    let table = accounts(&cluster);
    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    order(&mut cluster, PHONE, "one", &ada);
    cluster.settle();

    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    let unreferred = Transaction::new().delete_unreferred(table, &ada, placed(&cluster));
    assert!(!happened(&mut cluster, PHONE, unreferred.clone()));
    assert!(account(&cluster, NAS, "ada").is_some());
    assert_eq!(order_keys(&cluster, NAS), ["one"]);

    // The order goes. The account was still referred to once, which
    // is all a device can know on its own.
    let one = row(&cluster, PHONE, orders(&cluster), "one").expect("placed");
    cluster.write(PHONE, Transaction::new().delete_row(orders(&cluster), &one));
    cluster.settle();
    assert!(
        account(&cluster, PHONE, "ada")
            .expect("there")
            .is_referred()
    );

    assert!(happened(&mut cluster, PHONE, unreferred));
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(account(&cluster, device, "ada"), None, "device {device}");
    }
}

#[test]
fn a_reference_written_apart_from_it_doesnt_outlive_the_row() {
    let (mut cluster, _) = shop();
    let table = accounts(&cluster);
    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    cluster.isolate(NAS);
    order(&mut cluster, NAS, "late", &ada);

    let unreferred = Transaction::new().delete_unreferred(table, &ada, placed(&cluster));
    cluster.write(PHONE, unreferred);
    cluster.connect_all();
    cluster.settle();

    // Either the order came first and the account stays, or the
    // account went and the order with nothing to refer to never was.
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let kept = account(&cluster, device, "ada").is_some();
        let orders = order_keys(&cluster, device);
        assert_eq!(orders.len(), usize::from(kept), "device {device}");
    }
}

#[test]
fn referrers_are_only_looked_for_in_the_same_clock() {
    let (mut cluster, group) = shop();
    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    let far = ModelId::table(ClockId::new(group, [9; 32]), [8; 32]);
    let table = accounts(&cluster);

    let closing = Transaction::new().delete_with(table, &ada, [(far, buyer(far))]);

    assert!(!happened(&mut cluster, PHONE, closing));
    assert!(account(&cluster, NAS, "ada").is_some());
}

#[test]
fn a_column_that_cant_name_a_row_is_refused() {
    let (mut cluster, _) = shop();
    let ada = account(&cluster, PHONE, "ada").expect("signed up");
    let (table, orders) = (accounts(&cluster), orders(&cluster));
    let count = ColumnId::integer(orders, [5; 32]);

    let closing = Transaction::new().delete_with(table, &ada, [(orders, count)]);

    assert!(!happened(&mut cluster, PHONE, closing));
    assert!(account(&cluster, NAS, "ada").is_some());
}
