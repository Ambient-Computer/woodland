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
    assert_eq!(sim.devices[PHONE].device_secret(), secret(PHONE));
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
    sim.changes[PHONE].clear();
    sim.write_to(PHONE, group, Transaction::new().drop_model(notes(group)));

    // Nothing goes until the drop is committed, in either read.
    for read in [Sim::shown, Sim::committed] {
        let todo = read(&sim, PHONE, group, notes(group), "todo");
        assert_eq!(todo.as_deref(), Some("milk"));
    }
    let pending = block_on(sim.devices[PHONE].pending(group)).expect("read");
    assert_eq!(pending.len(), 1);
    assert_eq!(
        (pending[0].model, &pending[0].change),
        (notes(group), &crate::db::Waiting::Drop)
    );
    let everything = Changed::Everything { group };
    assert!(!sim.changes[PHONE].contains(&everything));

    // What's written after it shows too, until the drop is final.
    sim.write_to(PHONE, group, set(notes(group), "todo", "bread"));
    let shown = sim.shown(PHONE, group, notes(group), "todo");
    assert_eq!(shown.as_deref(), Some("bread"));

    sim.settle();
    assert!(sim.changes[PHONE].contains(&everything));
    let pending = block_on(sim.devices[PHONE].pending(group)).expect("read");
    assert_eq!(pending, []);
    sim.write_to(PHONE, group, set(notes(group), "todo", "eggs"));
    sim.settle();

    assert_eq!(sim.shown(PHONE, group, notes(group), "todo"), None);
    assert_eq!(sim.committed(PHONE, group, notes(group), "todo"), None);
}

#[test]
fn a_drop_that_doesnt_happen_takes_nothing() {
    let (mut sim, group) = Sim::group(1, 1);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();

    let dropping = Transaction::new()
        .expect(notes(group), "todo", "eggs")
        .drop_model(notes(group));
    sim.write_to(PHONE, group, dropping);
    sim.settle();

    assert_eq!(
        sim.committed(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
    let pending = block_on(sim.devices[PHONE].pending(group)).expect("read");
    assert_eq!(pending, []);
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

/// A model's ID is a hash, and one in 256 ends in `0xFF`. Its keys are
/// a range all the same.
#[test]
fn a_model_whose_id_ends_in_the_last_byte_is_read() {
    let (mut sim, group) = Sim::group(1, 1);
    let ends_last = |model: &ModelId| model.as_bytes().ends_with(&[0xFF]);
    let nonces = (0..=u16::MAX).map(|nonce| [nonce.to_be_bytes(); 16].concat());
    let models = nonces.map(|nonce| ModelId::kv(group, nonce.try_into().expect("32 bytes")));
    let model = models.into_iter().find(ends_last).expect("one in 256");
    let everything = Query::prefix(group, model, b"");

    sim.write_to(PHONE, group, set(model, "todo", "milk"));

    assert_eq!(sim.read(PHONE, &everything).len(), 1);
    assert_eq!(sim.read(PHONE, &everything.clone().committed()), []);

    sim.settle();

    assert_eq!(sim.read(PHONE, &everything.committed()).len(), 1);
}

/// A group's ID is a hash too, and everything a group owns is a range
/// that starts with it: what a device that forgets the group deletes.
#[test]
fn a_group_whose_id_ends_in_the_last_byte_is_forgotten() {
    let mut sim = Sim::new(2);
    let mut seeds = (0..=u16::MAX).map(|seed| [seed.to_be_bytes(); 16].concat());
    let group = loop {
        let seed = seeds.next().expect("one in 256");
        let founded = block_on(sim.devices[PHONE].found(seed.try_into().expect("32 bytes")));
        let (group, outputs) = founded.expect("founded");
        sim.absorb(PHONE, outputs);
        if group.as_bytes().ends_with(&[0xFF]) {
            break group;
        }
    };
    let invitation = sim.devices[PHONE].invite(group).expect("held");
    let (_, outputs) = block_on(sim.devices[LAPTOP].join(&invitation)).expect("joined");
    sim.absorb(LAPTOP, outputs);

    let outputs = block_on(sim.devices[LAPTOP].forget(group)).expect("forgotten");
    sim.absorb(LAPTOP, outputs);
    sim.restart(LAPTOP);

    assert_eq!(sim.devices[LAPTOP].groups().count(), 0);
}
