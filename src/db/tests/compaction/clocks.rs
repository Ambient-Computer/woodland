//! Shedding one clock's history and keeping another's.

use super::*;

fn photos(group: GroupId) -> ClockId {
    ClockId::new(group, [7; 32])
}

fn captions(group: GroupId) -> ModelId {
    ModelId::kv(photos(group), [1; 32])
}

/// How many entries of `clock` the device keeps.
fn entries(sim: &Sim, device: usize, group: GroupId, clock: ClockId) -> usize {
    let prefix = [&keys::logs(group)[..], clock.as_bytes()].concat();
    let (start, end) = keys::range(prefix);
    let view = sim.devices[device].store.view();

    block_on(crate::db::step::scan(&view, &start, &end))
        .expect("scanned")
        .len()
}

fn compact_clock(sim: &mut Sim, device: usize, group: GroupId, clock: ClockId) {
    let outputs = block_on(sim.devices[device].compact_clock(group, clock)).expect("compacted");
    sim.absorb(device, outputs);
}

fn want(sim: &mut Sim, device: usize, group: GroupId, clocks: &[ClockId]) {
    let clocks = clocks.iter().copied().collect();
    let outputs = block_on(sim.devices[device].want(group, Some(clocks))).expect("wanted");
    sim.absorb(device, outputs);
}

/// Notes and captions, committed over several checkpoints while the
/// friend is away. The phone then sheds the notes' history and keeps
/// the captions'.
fn half_shed() -> (Sim, GroupId) {
    let (mut sim, group) = Sim::group(4, 3);
    sim.settle();
    sim.isolate(FRIEND);
    for round in 0..3 {
        let key = format!("{round}");
        sim.write_to(PHONE, group, set(notes(group), &key, "note"));
        sim.write_to(LAPTOP, group, set(captions(group), &key, "caption"));
        sim.settle();
    }
    compact_clock(&mut sim, PHONE, group, group.into());

    (sim, group)
}

#[test]
fn one_clocks_history_is_shed_and_anothers_kept() {
    let (sim, group) = half_shed();

    assert_eq!(entries(&sim, PHONE, group, group.into()), 0);
    assert_eq!(entries(&sim, PHONE, group, photos(group)), 3);
    for round in ["0", "1", "2"] {
        let note = sim.committed(PHONE, group, notes(group), round);
        assert_eq!(note.as_deref(), Some("note"));
        let caption = sim.committed(PHONE, group, captions(group), round);
        assert_eq!(caption.as_deref(), Some("caption"));
    }
}

#[test]
fn a_peer_that_wants_the_kept_clock_is_sent_its_history() {
    let (mut sim, group) = half_shed();

    want(&mut sim, FRIEND, group, &[photos(group)]);
    sim.connect(FRIEND, PHONE);
    sim.settle();

    assert_eq!(entries(&sim, FRIEND, group, photos(group)), 3);
    let caption = sim.committed(FRIEND, group, captions(group), "2");
    assert_eq!(caption.as_deref(), Some("caption"));
}

#[test]
fn a_peer_that_wants_the_shed_clock_too_is_sent_state() {
    let (mut sim, group) = half_shed();
    // It's still sent state after the phone restarts.
    sim.restart(PHONE);

    sim.connect(FRIEND, PHONE);
    sim.settle();

    assert_eq!(entries(&sim, FRIEND, group, photos(group)), 0);
    for model in [notes(group), captions(group)] {
        assert!(sim.committed(FRIEND, group, model, "2").is_some());
    }
}

#[test]
fn shedding_every_clock_one_at_a_time_is_shedding_them_all() {
    let (mut sim, group) = half_shed();
    compact_clock(&mut sim, PHONE, group, photos(group));

    assert_eq!(sim.entries_kept(PHONE, group), 0);
    want(&mut sim, FRIEND, group, &[photos(group)]);
    sim.connect(FRIEND, PHONE);
    sim.settle();
    assert_eq!(entries(&sim, FRIEND, group, photos(group)), 0);
    let caption = sim.committed(FRIEND, group, captions(group), "2");
    assert_eq!(caption.as_deref(), Some("caption"));
}
