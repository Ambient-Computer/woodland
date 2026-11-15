use super::*;

#[test]
fn a_write_is_shown_at_once_and_committed_by_a_checkpoint() {
    let (mut sim, group) = Sim::group(1, 1);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));

    assert_eq!(
        sim.shown(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
    assert_eq!(sim.committed(PHONE, group, notes(group), "todo"), None);

    sim.settle();

    assert_eq!(
        sim.shown(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
    assert_eq!(
        sim.committed(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
}

#[test]
fn a_later_write_replaces_an_earlier_one() {
    let (mut sim, group) = Sim::group(1, 1);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();
    sim.write_to(PHONE, group, set(notes(group), "todo", "eggs"));

    assert_eq!(
        sim.shown(PHONE, group, notes(group), "todo").as_deref(),
        Some("eggs")
    );
    assert_eq!(
        sim.committed(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );

    sim.write_to(
        PHONE,
        group,
        Transaction::new().remove(notes(group), "todo"),
    );
    sim.settle();

    assert_eq!(sim.shown(PHONE, group, notes(group), "todo"), None);
    assert_eq!(sim.committed(PHONE, group, notes(group), "todo"), None);
}

#[test]
fn a_device_picks_up_where_it_stopped() {
    let (mut sim, group) = Sim::group(1, 1);
    sim.write_to(PHONE, group, set(notes(group), "kept", "committed"));
    sim.settle();
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    let before = sim.id(PHONE);

    sim.restart(PHONE);

    assert_eq!(sim.id(PHONE), before, "the key is the one in the store");
    assert_eq!(
        sim.committed(PHONE, group, notes(group), "kept").as_deref(),
        Some("committed")
    );
    assert_eq!(
        sim.shown(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
    assert_eq!(sim.committed(PHONE, group, notes(group), "todo"), None);

    sim.write_to(PHONE, group, set(notes(group), "more", "after"));
    sim.settle();

    assert_eq!(
        sim.committed(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
    assert_eq!(
        sim.committed(PHONE, group, notes(group), "more").as_deref(),
        Some("after")
    );
}

#[test]
fn a_transaction_whose_expectation_fails_does_nothing() {
    let (mut sim, group) = Sim::group(1, 1);
    sim.write_to(PHONE, group, set(notes(group), "count", "1"));

    let stale = Transaction::new()
        .expect(notes(group), "count", "0")
        .set(notes(group), "count", "2")
        .set(notes(group), "other", "written");
    sim.write_to(PHONE, group, stale);
    let fresh =
        Transaction::new()
            .expect(notes(group), "count", "1")
            .set(notes(group), "count", "2");
    sim.write_to(PHONE, group, fresh);

    assert_eq!(
        sim.shown(PHONE, group, notes(group), "count").as_deref(),
        Some("2")
    );
    assert_eq!(sim.shown(PHONE, group, notes(group), "other"), None);

    sim.settle();

    assert_eq!(
        sim.committed(PHONE, group, notes(group), "count")
            .as_deref(),
        Some("2")
    );
    assert_eq!(sim.committed(PHONE, group, notes(group), "other"), None);
}

#[test]
fn a_dropped_model_is_empty_for_good() {
    let (mut sim, group) = Sim::group(1, 1);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();
    sim.write_to(PHONE, group, Transaction::new().drop_model(notes(group)));

    assert_eq!(sim.shown(PHONE, group, notes(group), "todo"), None);
    assert_eq!(
        sim.committed(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );

    sim.settle();
    sim.write_to(PHONE, group, set(notes(group), "todo", "eggs"));
    sim.settle();

    assert_eq!(sim.shown(PHONE, group, notes(group), "todo"), None);
    assert_eq!(sim.committed(PHONE, group, notes(group), "todo"), None);
}

#[test]
fn readers_hear_what_changed() {
    let (mut sim, group) = Sim::group(1, 1);
    sim.changes[PHONE].clear();
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));

    let optimistic = Query::key(group, notes(group), b"todo");
    let committed = optimistic.clone().committed();
    let affects =
        |query: &Query, changes: &[Changed]| changes.iter().any(|it| query.is_affected_by(it));
    assert!(affects(&optimistic, &sim.changes[PHONE]));
    assert!(!affects(&committed, &sim.changes[PHONE]));

    sim.changes[PHONE].clear();
    sim.settle();

    assert!(affects(&committed, &sim.changes[PHONE]));
}

#[test]
fn only_a_contributor_writes() {
    let (mut sim, group) = Sim::group(2, 1);

    let refused = block_on(sim.devices[LAPTOP].write(group, set(notes(group), "todo", "milk")));

    assert!(matches!(refused, Err(Error::NotAContributor)));
}
