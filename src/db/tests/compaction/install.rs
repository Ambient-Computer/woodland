//! A snapshot with more in it than a step puts in place.

use std::collections::BTreeSet;

use super::*;

const EACH: usize = 40;

/// Enough that a snapshot of it comes in several pieces.
const MANY: usize = 200;

/// More than a step looks at, so that what's there already runs past
/// the end of a page.
const STALE: usize = 12;

fn photos(group: GroupId) -> ClockId {
    ClockId::new(group, [7; 32])
}

fn captions(group: GroupId) -> ModelId {
    ModelId::kv(photos(group), [1; 32])
}

fn want(sim: &mut Sim, group: GroupId, clocks: Option<&[ClockId]>) -> Result<(), Error> {
    let clocks: Option<BTreeSet<ClockId>> = clocks.map(|it| it.iter().copied().collect());
    let outputs = block_on(sim.devices[NAS].want(group, clocks))?;
    sim.absorb(NAS, outputs);

    Ok(())
}

fn committed(sim: &Sim, device: usize, group: GroupId, model: ModelId) -> Vec<(Vec<u8>, Vec<u8>)> {
    sim.read(device, &Query::prefix(group, model, b"").committed())
}

/// The phone and the laptop decide, and the NAS follows. It has
/// committed state of its own when it falls behind what they've shed,
/// and has just made a start on the snapshot that replaces it.
fn started(only_notes: bool, each: usize) -> (Sim, GroupId) {
    let (mut sim, group) = Sim::group(3, 2);
    sim.write_to(PHONE, group, set(notes(group), "z stale", "old"));
    sim.write_to(PHONE, group, set(notes(group), "note 0001", "old"));
    let stale = |index: usize| format!("a stale {index}");
    for index in 0..STALE {
        for model in [notes(group), captions(group)] {
            sim.write_to(PHONE, group, set(model, &stale(index), "old"));
        }
    }
    sim.settle();
    if only_notes {
        want(&mut sim, group, Some(&[notes(group).clock()])).expect("wanted");
        sim.settle();
    }

    sim.isolate(NAS);
    let gone = Transaction::new().remove(notes(group), "z stale");
    sim.write_to(PHONE, group, gone);
    for index in 0..STALE {
        for model in [notes(group), captions(group)] {
            let gone = Transaction::new().remove(model, stale(index));
            sim.write_to(PHONE, group, gone);
        }
    }
    for index in 0..each {
        let name = format!("note {index:04}");
        sim.write_to(PHONE, group, set(notes(group), &name, "written"));
        sim.write_to(PHONE, group, set(captions(group), &name, "untitled"));
    }
    sim.settle();
    sim.compact(PHONE, group);
    sim.compact(LAPTOP, group);
    let height = sim.devices[PHONE].groups[&group].folded();

    sim.connect(NAS, PHONE);
    while sim.devices[NAS].groups[&group].reached() < height {
        assert!(sim.deliver(), "it never got the snapshot");
    }
    assert!(sim.devices[NAS].groups[&group].folded() < height);

    (sim, group)
}

#[test]
fn a_big_snapshot_is_put_in_place_a_few_values_at_a_time() {
    let (mut sim, group) = started(false, EACH);

    // Some of it is in place. What it replaces is partly still there.
    let partway = committed(&sim, NAS, group, notes(group));
    assert!(partway.len() < EACH);
    assert_eq!(
        sim.committed(NAS, group, notes(group), "z stale")
            .as_deref(),
        Some("old")
    );

    // It carries on from its store alone, and ends up with all of it
    // and nothing else.
    sim.restart(NAS);
    assert!(sim.resume());
    assert!(committed(&sim, NAS, group, notes(group)).len() < EACH);
    while sim.resume() {}

    for model in [notes(group), captions(group)] {
        assert_eq!(committed(&sim, NAS, group, model).len(), EACH);
        assert_eq!(
            committed(&sim, NAS, group, model),
            committed(&sim, PHONE, group, model)
        );
    }

    // And it's the state the others have: what comes next commits.
    sim.connect(NAS, PHONE);
    sim.write_to(PHONE, group, set(notes(group), "late", "arrival"));
    sim.settle();

    assert_eq!(
        sim.committed(NAS, group, notes(group), "late").as_deref(),
        Some("arrival")
    );
    assert_eq!(
        sim.devices[NAS].groups[&group].folded(),
        sim.devices[PHONE].groups[&group].folded()
    );
}

/// The connection drops, and the sender starts the snapshot again. What's
/// being put in place is whole already, and isn't thrown away.
#[test]
fn a_snapshot_sent_again_doesnt_disturb_one_being_put_in_place() {
    let (mut sim, group) = started(false, MANY);
    sim.disconnect(NAS, PHONE);
    sim.connect(NAS, PHONE);
    sim.run();

    for model in [notes(group), captions(group)] {
        assert_eq!(committed(&sim, NAS, group, model).len(), MANY);
    }
    sim.write_to(PHONE, group, set(notes(group), "late", "arrival"));
    sim.settle();

    assert_eq!(
        sim.committed(NAS, group, notes(group), "late").as_deref(),
        Some("arrival")
    );
}

#[test]
fn a_clock_let_go_partway_through_a_snapshot_stays_gone() {
    let (mut sim, group) = started(false, EACH);
    want(&mut sim, group, Some(&[notes(group).clock()])).expect("wanted");
    sim.run();

    assert_eq!(committed(&sim, NAS, group, notes(group)).len(), EACH);
    let (start, end) =
        keys::range([&keys::committed(group)[..], photos(group).as_bytes()].concat());
    let view = sim.devices[NAS].store.view();
    let kept = block_on(crate::db::step::scan(&view, &start, &end)).expect("scanned");
    assert_eq!(kept.len(), 0);

    sim.write_to(PHONE, group, set(notes(group), "late", "arrival"));
    sim.settle();

    assert_eq!(
        sim.committed(NAS, group, notes(group), "late").as_deref(),
        Some("arrival")
    );
}

/// The snapshot was asked for without the clock, so it has none of it.
#[test]
fn a_clock_cant_be_wanted_partway_through_a_snapshot_that_lacks_it() {
    let (mut sim, group) = started(true, EACH);

    assert!(matches!(
        want(&mut sim, group, None),
        Err(Error::PassedOver)
    ));

    sim.run();
    assert_eq!(committed(&sim, NAS, group, notes(group)).len(), EACH);
    assert!(matches!(
        want(&mut sim, group, None),
        Err(Error::PassedOver)
    ));
}
