//! Transactions that depend on what their author read, and locks.

use super::*;
use crate::model::Value;

/// Writes a new count, and who wrote it, if the count is still `from`.
pub(super) fn count(cluster: &mut Sim, device: usize, from: Option<&str>, to: &str) {
    let notes = household_notes(cluster);
    let (from, to) = (from.map(str::to_owned), to.to_owned());
    cluster.write(device, {
        let transaction = Transaction::new();
        let guarded = match from {
            Some(from) => transaction.expect(notes, "count", from),
            None => transaction.expect_absent(notes, "count"),
        };

        guarded
            .set(notes, "count", to.clone())
            .set(notes, "counted", to)
    });
}

#[test]
fn a_write_happens_if_what_it_expected_still_holds() {
    let mut cluster = household();
    count(&mut cluster, PHONE, None, "1");
    cluster.settle();
    count(&mut cluster, LAPTOP, Some("1"), "2");
    count(&mut cluster, LAPTOP, Some("2"), "3");
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(read(&cluster, device, "count").as_deref(), Some("3"));
        assert_eq!(
            read_committed(&cluster, device, "count").as_deref(),
            Some("3")
        );
    }
}

#[test]
fn a_write_that_expected_something_else_never_happens() {
    let mut cluster = household();
    count(&mut cluster, PHONE, None, "1");
    cluster.settle();
    count(&mut cluster, LAPTOP, Some("7"), "8");
    count(&mut cluster, LAPTOP, None, "9");
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(read(&cluster, device, "count").as_deref(), Some("1"));
        assert_eq!(read(&cluster, device, "counted").as_deref(), Some("1"));
    }
}

/// Two devices read the same count while apart, and each writes the
/// next one. Only one of them can have: the other's whole transaction,
/// and everything it built on top, never happened.
#[test]
fn only_one_of_two_writes_from_the_same_read_happens() {
    let mut cluster = household();
    count(&mut cluster, PHONE, None, "1");
    cluster.settle();
    cluster.disconnect_all();
    cluster.settle();

    count(&mut cluster, PHONE, Some("1"), "phone");
    count(&mut cluster, PHONE, Some("phone"), "phone again");
    count(&mut cluster, LAPTOP, Some("1"), "laptop");
    count(&mut cluster, LAPTOP, Some("laptop"), "laptop again");
    cluster.settle();
    assert_eq!(
        read(&cluster, PHONE, "count").as_deref(),
        Some("phone again")
    );
    assert_eq!(
        read(&cluster, LAPTOP, "count").as_deref(),
        Some("laptop again")
    );

    cluster.connect_all();
    cluster.settle();

    let winner = read(&cluster, PHONE, "count");
    assert!(matches!(
        winner.as_deref(),
        Some("phone again" | "laptop again")
    ));
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(read(&cluster, device, "count"), winner, "device {device}");
        assert_eq!(read(&cluster, device, "counted"), winner);
        assert_eq!(read_committed(&cluster, device, "count"), winner);
        assert_eq!(stored_frontier(&cluster, device), [], "device {device}");
    }
}

/// Writes a note that only happens if `owner` holds the lock.
fn write_locked(cluster: &mut Sim, device: usize, owner: &str, key: &str) {
    let (names, notes) = (usernames(cluster), household_notes(cluster));
    let (owner, key) = (owner.to_owned(), key.to_owned());
    cluster.write(device, {
        let transaction = Transaction::new();

        transaction
            .expect(names, "lock", owner.clone())
            .set(notes, key, owner)
    });
}

fn release(cluster: &mut Sim, device: usize, owner: &str) {
    let names = usernames(cluster);
    let owner = owner.to_owned();
    cluster.write(device, {
        let transaction = Transaction::new();

        transaction.release(names, "lock", owner)
    });
}

/// Two devices take the same lock while apart, and each goes on to
/// work under it. Whoever didn't get it did none of that work.
#[test]
fn work_under_a_lock_goes_with_the_lock() {
    let mut cluster = household();
    cluster.disconnect_all();
    cluster.settle();

    for (device, owner) in [(PHONE, "phone"), (LAPTOP, "laptop")] {
        claim_name(&mut cluster, device, "lock", owner);
        write_locked(&mut cluster, device, owner, "first");
        write_locked(&mut cluster, device, owner, owner);
    }

    cluster.settle();
    assert_eq!(read(&cluster, LAPTOP, "first").as_deref(), Some("laptop"));

    cluster.connect_all();
    cluster.settle();

    let winner = holder(&cluster, PHONE, "lock").expect("someone holds the lock");
    let loser = if winner == "phone" { "laptop" } else { "phone" };
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(read(&cluster, device, "first").as_ref(), Some(&winner));
        assert_eq!(read(&cluster, device, &winner).as_ref(), Some(&winner));
        assert_eq!(read(&cluster, device, loser), None, "device {device}");
        assert_eq!(stored_frontier(&cluster, device), [], "device {device}");
    }
}

#[test]
fn a_released_lock_can_be_taken_again() {
    let mut cluster = household();
    claim_name(&mut cluster, PHONE, "lock", "phone");
    cluster.settle();

    // Only its holder can release it.
    release(&mut cluster, LAPTOP, "laptop");
    cluster.settle();
    assert_eq!(holder(&cluster, NAS, "lock").as_deref(), Some("phone"));

    release(&mut cluster, PHONE, "phone");
    write_locked(&mut cluster, PHONE, "phone", "late");
    cluster.settle();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(holder(&cluster, device, "lock"), None, "device {device}");
        assert_eq!(read(&cluster, device, "late"), None, "device {device}");
    }

    claim_name(&mut cluster, LAPTOP, "lock", "laptop");
    write_locked(&mut cluster, LAPTOP, "laptop", "late");
    cluster.settle();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(holder(&cluster, device, "lock").as_deref(), Some("laptop"));
        assert_eq!(read(&cluster, device, "late").as_deref(), Some("laptop"));
    }
}

/// Makes the person in a row older, if nothing about the row has
/// changed since it was read.
fn age_row(cluster: &mut Sim, device: usize, table: ModelId, row: &crate::Row, years: i64) {
    let cells = [(tables::age(table), Value::from(years))];
    cluster.write(device, {
        let transaction = Transaction::new();

        transaction
            .expect_unchanged(table, row)
            .update(table, row, cells)
    });
}

fn ada(cluster: &Sim, device: usize, table: ModelId) -> crate::Row {
    let table = tables::read_table(cluster, device, table).expect("the table exists");

    table.row(b"ada").expect("the row exists").clone()
}

/// Changes to a row don't mind each other unless they're asked to.
/// This one is: if someone else wrote to the row in between, in any
/// column, it doesn't happen.
#[test]
fn a_row_can_be_changed_only_if_its_what_was_read() {
    let mut cluster = household();
    let table = tables::people(&cluster, 1);
    tables::create(&mut cluster, PHONE, table);
    tables::insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();

    let stale = ada(&cluster, PHONE, table);
    let fresh = ada(&cluster, LAPTOP, table);
    age_row(&mut cluster, LAPTOP, table, &fresh, 37);
    cluster.settle();
    age_row(&mut cluster, PHONE, table, &stale, 40);
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let row = ada(&cluster, device, table);
        assert_eq!(row.get(tables::age(table)), &Value::from(37));
    }

    let fresh = ada(&cluster, PHONE, table);
    age_row(&mut cluster, PHONE, table, &fresh, 40);
    cluster.settle();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let row = ada(&cluster, device, table);
        assert_eq!(row.get(tables::age(table)), &Value::from(40));
    }
}
