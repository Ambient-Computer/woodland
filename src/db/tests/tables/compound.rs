//! An index of several columns: rows by what they hold in the first,
//! and then in the next.

use super::indexes::{found, set_plan, sign_up_at, signed_up};
use std::ops::Bound;

use super::unique::{account, plan, tier, update};
use super::*;

/// Four accounts on two plans, and one that's on none.
fn on_plans() -> (Sim, GroupId, ModelId) {
    let (mut cluster, group, table) = signed_up();
    for (key, name) in [
        ("ada", "paid"),
        ("al", "paid"),
        ("alan", "free"),
        ("grace", "paid"),
    ] {
        set_plan(&mut cluster, PHONE, key, name);
    }

    sign_up_at(&mut cluster, PHONE, "zed", 5);
    cluster.settle();

    (cluster, group, table)
}

#[test]
fn rows_are_found_by_one_column_and_then_the_next() {
    let (mut cluster, group, table) = on_plans();
    let by = [plan(table), tier(table)];
    cluster.keep_index(NAS, table, by);
    assert!(cluster.devices[NAS].has_index(group, table, by));
    let find = |held: &[Value], tiers: (Bound<Value>, Bound<Value>)| {
        let query = Query::index_within(group, table, by, held, tiers);

        found(&cluster, NAS, &query)
    };
    let all = (Bound::Unbounded, Bound::Unbounded);
    let from = |tier: i64| (Bound::Included(Value::from(tier)), Bound::Unbounded);

    // Everyone on a plan, by tier.
    let paid = [Value::from("paid")];
    assert_eq!(find(&paid, all.clone()), ["ada", "grace", "al"]);
    assert_eq!(find(&paid, from(2)), ["grace", "al"]);
    assert_eq!(find(&paid, from(10)), [""; 0]);
    assert_eq!(find(&["free".into()], from(2)), ["alan"]);

    // A value for every column is one place in the index.
    assert_eq!(find(&["paid".into(), 2.into()], all.clone()), ["grace"]);

    // No values, and the range is of the first column. Whoever holds
    // nothing in one of the columns isn't in the index.
    assert_eq!(find(&[], all), ["alan", "ada", "grace", "al"]);
    let free = Value::from("free");
    let only = (Bound::Included(free.clone()), Bound::Included(free));
    assert_eq!(find(&[], only), ["alan"]);

    // The columns the other way round are another index, and this
    // device doesn't keep it.
    let other = [tier(table), plan(table)];
    assert!(!cluster.devices[NAS].has_index(group, table, other));
    let query = Query::index_within(group, table, other, [], ..);
    assert_eq!(found(&cluster, NAS, &query), [""; 0]);
}

/// A row moves in the index when either column changes, at each level
/// of read as that level sees it.
#[test]
fn a_row_moves_when_either_column_changes() {
    let (mut cluster, group, table) = on_plans();
    let by = [plan(table), tier(table)];
    cluster.keep_index(LAPTOP, table, by);
    cluster.disconnect_all();
    let on = |cluster: &Sim, name: &str, committed: bool| {
        let query = Query::index_within(group, table, by, [Value::from(name)], ..);
        let query = if committed { query.committed() } else { query };

        found(cluster, LAPTOP, &query)
    };

    set_plan(&mut cluster, LAPTOP, "grace", "free");
    let al = account(&cluster, LAPTOP, "al").expect("signed up");
    update(&mut cluster, LAPTOP, &al, tier(table), 0);
    set_plan(&mut cluster, LAPTOP, "zed", "paid");
    let ada = account(&cluster, LAPTOP, "ada").expect("signed up");
    update(&mut cluster, LAPTOP, &ada, tier(table), Value::Null);

    assert_eq!(on(&cluster, "paid", false), ["al", "zed"]);
    assert_eq!(on(&cluster, "free", false), ["grace", "alan"]);
    assert_eq!(on(&cluster, "paid", true), ["ada", "grace", "al"]);
    assert_eq!(on(&cluster, "free", true), ["alan"]);

    cluster.connect_all();
    cluster.settle();
    assert_eq!(on(&cluster, "paid", true), ["al", "zed"]);
    assert_eq!(on(&cluster, "free", true), ["grace", "alan"]);
}

/// An index of one column and an index of that column and another are
/// two indexes. Dropping one leaves the other.
#[test]
fn indexes_that_share_a_column_are_kept_apart() {
    let (mut cluster, group, table) = on_plans();
    let both = [plan(table), tier(table)];
    cluster.keep_index(NAS, table, both);
    cluster.keep_index(NAS, table, plan(table));
    cluster.changes[NAS].clear();
    let by_plan = Query::lookup(group, table, plan(table), &"paid".into());
    let by_both = Query::index_within(group, table, both, [Value::from("paid")], ..);
    assert_eq!(found(&cluster, NAS, &by_plan), ["ada", "al", "grace"]);
    assert_eq!(found(&cluster, NAS, &by_both), ["ada", "grace", "al"]);

    cluster.wake(NAS);
    let outputs = block_on(cluster.devices[NAS].drop_index(group, table, plan(table)));
    cluster.absorb(NAS, outputs.expect("dropped"));
    cluster.run();
    assert!(cluster.devices[NAS].has_index(group, table, both));
    assert!(!cluster.devices[NAS].has_index(group, table, plan(table)));
    assert_eq!(found(&cluster, NAS, &by_plan), [""; 0]);
    assert_eq!(found(&cluster, NAS, &by_both), ["ada", "grace", "al"]);

    // A query is told when its own index is full, and not another's.
    cluster.keep_index(NAS, table, plan(table));
    let full = |change: &&Changed| matches!(change, Changed::Indexed { .. });
    let told: Vec<_> = cluster.changes[NAS].iter().filter(full).collect();
    assert_eq!(told.len(), 1);
    assert!(by_plan.is_affected_by(told[0]));
    assert!(!by_both.is_affected_by(told[0]));
}

/// Dropping a column empties every index it's in.
#[test]
fn an_index_is_empty_once_one_of_its_columns_is_dropped() {
    let (mut cluster, group, table) = on_plans();
    let by = [plan(table), tier(table)];
    cluster.keep_index(NAS, table, by);
    cluster.write(PHONE, Transaction::new().drop_column(table, tier(table)));
    cluster.settle();

    let query = Query::index_within(group, table, by, [], ..);
    assert_eq!(found(&cluster, NAS, &query), [""; 0]);
    assert_eq!(found(&cluster, NAS, &query.committed()), [""; 0]);
}
