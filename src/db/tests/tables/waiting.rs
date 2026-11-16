//! Seeing that a change is waiting to be committed.

use super::*;
use crate::db::Waiting;
use crate::model::Filter;

fn number(table: ModelId) -> ColumnId {
    ColumnId::integer(table, [7; 32])
}

/// A table of people everyone knows of, and nobody connected.
fn apart() -> (Sim, ModelId) {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create(&mut cluster, PHONE, table);
    cluster.write(
        PHONE,
        Transaction::new().add_column(table, number(table), "number"),
    );
    insert(&mut cluster, PHONE, table, "ada", 36);
    cluster.settle();
    cluster.disconnect_all();

    (cluster, table)
}

fn waiting(cluster: &Sim, device: usize) -> Vec<Waiting> {
    let pending = block_on(cluster.devices[device].pending(cluster.home)).expect("read");

    pending.into_iter().map(|it| it.change).collect()
}

fn told(cluster: &Sim, device: usize, table: ModelId) -> usize {
    let group = cluster.home;
    let ours = |change: &&Changed| {
        **change
            == Changed::Pending {
                group,
                model: table,
            }
    };

    cluster.changes[device].iter().filter(ours).count()
}

#[test]
fn a_change_is_pending_from_when_its_written_until_its_committed() {
    let (mut cluster, table) = apart();
    assert_eq!(waiting(&cluster, PHONE), []);

    let numbered = Transaction::new().insert_numbered(table, "grace", [], number(table));
    cluster.write(PHONE, numbered);

    let expected = Waiting::Number {
        row: b"grace".to_vec(),
        column: number(table),
    };
    assert_eq!(waiting(&cluster, PHONE), std::slice::from_ref(&expected));
    assert_eq!(waiting(&cluster, LAPTOP), []);
    assert_eq!(told(&cluster, PHONE, table), 1);
    let pending = block_on(cluster.devices[PHONE].pending(cluster.home)).expect("read");
    assert_eq!(
        (pending[0].model, pending[0].author),
        (table, cluster.id(PHONE))
    );

    // Heard of, and not committed: two of three can't be reached.
    cluster.connect(PHONE, FRIEND);
    cluster.settle();
    assert_eq!(waiting(&cluster, FRIEND), [expected]);
    assert_eq!(told(&cluster, FRIEND, table), 1);

    cluster.connect_all();
    cluster.settle();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(waiting(&cluster, device), [], "device {device}");
    }
    assert!(told(&cluster, PHONE, table) > 1);
    assert!(told(&cluster, FRIEND, table) > 1);
}

#[test]
fn every_kind_of_change_that_waits_is_listed_in_order() {
    let (mut cluster, table) = apart();
    let ada = row(&cluster, PHONE, table, "ada").expect("inserted");
    let old = Filter::all().at_least(age(table), 65);

    cluster.write(PHONE, {
        let transaction = Transaction::new();

        transaction
            .update_where(table, old.clone(), [(age(table), 65.into())])
            .delete_where(table, old.clone())
    });
    cluster.write(PHONE, Transaction::new().delete_with(table, &ada, []));
    cluster.write(PHONE, Transaction::new().delete_unreferred(table, &ada, []));

    let row = b"ada".to_vec();
    assert_eq!(
        waiting(&cluster, PHONE),
        [
            Waiting::Update {
                filter: old.clone()
            },
            Waiting::DeleteWhere { filter: old },
            Waiting::Delete {
                row: row.clone(),
                with_referrers: true
            },
            Waiting::Delete {
                row,
                with_referrers: false
            },
        ]
    );
}

#[test]
fn a_change_in_a_transaction_that_doesnt_count_isnt_pending() {
    let (mut cluster, table) = apart();
    let notes = household_notes(&cluster);

    // Written after something else, so it's placed after whatever
    // another device writes first.
    write(&mut cluster, PHONE, "busy", "yes");
    cluster.write(PHONE, {
        let transaction = Transaction::new().expect(notes, "open", "yes");

        transaction.delete_where(table, Filter::all())
    });
    assert_eq!(waiting(&cluster, PHONE), []);

    // It counts once what it expected is there, written apart and
    // placed before it.
    cluster.write(LAPTOP, Transaction::new().set(notes, "open", "yes"));
    cluster.connect(PHONE, LAPTOP);
    let mut seen = false;
    while cluster.deliver() {
        seen |= waiting(&cluster, PHONE).len() == 1;
    }
    assert!(seen, "it was pending once the phone heard");

    // The two of them are enough to commit it, and then it's done.
    cluster.settle();
    assert_eq!(waiting(&cluster, PHONE), []);
    assert_eq!(ages(&cluster, PHONE, table), []);
}

#[test]
fn what_is_pending_survives_a_restart() {
    let (mut cluster, table) = apart();
    cluster.write(PHONE, Transaction::new().delete_where(table, Filter::all()));

    cluster.restart(PHONE);

    assert_eq!(waiting(&cluster, PHONE).len(), 1);
}

#[test]
fn a_group_nobody_here_holds_has_nothing_pending() {
    let cluster = household();
    let unknown = GroupId::from_bytes([9; 32]);

    let pending = block_on(cluster.devices[PHONE].pending(unknown));

    assert!(matches!(pending, Err(Error::UnknownGroup)));
}
