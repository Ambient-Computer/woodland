//! A group with more than one clock: separate histories, one chain.

use super::*;

fn photos(group: GroupId) -> ClockId {
    ClockId::new(group, [7; 32])
}

/// A model with the same nonce as [`notes`], in another clock.
fn captions(group: GroupId) -> ModelId {
    ModelId::kv(photos(group), [1; 32])
}

fn tags(group: GroupId) -> ModelId {
    ModelId::claims(photos(group), [2; 32])
}

#[test]
fn a_clock_exists_once_its_written_to() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.write_to(PHONE, group, set(captions(group), "beach", "sunset"));
    sim.write_to(LAPTOP, group, set(notes(group), "todo", "milk"));
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.committed(device, group, captions(group), "beach")
                .as_deref(),
            Some("sunset")
        );
        assert_eq!(
            sim.committed(device, group, notes(group), "todo")
                .as_deref(),
            Some("milk")
        );
    }
}

#[test]
fn models_in_different_clocks_are_different_models() {
    let (mut sim, group) = Sim::group(2, 2);
    assert_ne!(captions(group), notes(group));

    sim.write_to(PHONE, group, set(notes(group), "beach", "a note"));
    sim.write_to(PHONE, group, set(captions(group), "beach", "a caption"));
    sim.settle();

    assert_eq!(
        sim.committed(LAPTOP, group, notes(group), "beach")
            .as_deref(),
        Some("a note")
    );
    assert_eq!(
        sim.committed(LAPTOP, group, captions(group), "beach")
            .as_deref(),
        Some("a caption")
    );
}

#[test]
fn a_transaction_stays_within_one_clock() {
    let (mut sim, group) = Sim::group(1, 1);
    let both = Transaction::new().set(notes(group), "todo", "milk").set(
        captions(group),
        "beach",
        "sunset",
    );

    let refused = block_on(sim.devices[PHONE].write(group, both));

    assert!(matches!(refused, Err(Error::ManyClocks)));
    sim.settle();
    assert_eq!(sim.shown(PHONE, group, notes(group), "todo"), None);
    assert_eq!(sim.shown(PHONE, group, captions(group), "beach"), None);
}

/// Each clock has its own timeline. A device that has been busy in one
/// doesn't sort late in another because of it.
#[test]
fn being_busy_in_one_clock_doesnt_win_races_in_another() {
    let (mut sim, group) = Sim::group(2, 2);
    sim.disconnect_all();
    for round in 0..5 {
        sim.write_to(LAPTOP, group, set(notes(group), "busy", &round.to_string()));
    }
    sim.write_to(PHONE, group, claim(tags(group), "beach", "phone"));
    sim.write_to(LAPTOP, group, claim(tags(group), "beach", "laptop"));
    sim.connect_all();
    sim.settle();

    // Both claims are their authors' first entries in the clock, so the
    // tie goes to whichever key sorts first, however much either wrote
    // elsewhere.
    let first = sim.id(PHONE).as_bytes() < sim.id(LAPTOP).as_bytes();
    let winner = if first { "phone" } else { "laptop" };
    for device in [PHONE, LAPTOP] {
        assert_eq!(
            sim.committed(device, group, tags(group), "beach")
                .as_deref(),
            Some(winner)
        );
    }
}

#[test]
fn every_clock_comes_back_after_a_restart() {
    let (mut sim, group) = Sim::group(2, 2);
    sim.write_to(PHONE, group, set(captions(group), "beach", "sunset"));
    sim.settle();
    sim.isolate(PHONE);
    sim.write_to(PHONE, group, set(captions(group), "hill", "fog"));
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));

    sim.restart(PHONE);

    assert_eq!(
        sim.committed(PHONE, group, captions(group), "beach")
            .as_deref(),
        Some("sunset")
    );
    assert_eq!(
        sim.shown(PHONE, group, captions(group), "hill").as_deref(),
        Some("fog")
    );

    sim.connect_all();
    sim.settle();

    for model in [captions(group), notes(group)] {
        let query = Query::prefix(group, model, b"");
        assert_eq!(sim.read(LAPTOP, &query), sim.read(PHONE, &query));
        assert_eq!(
            sim.read(LAPTOP, &query.clone().committed()),
            sim.read(PHONE, &query)
        );
    }
}

/// What a step commits is counted across the group, not clock by
/// clock.
#[test]
fn a_step_commits_only_so_much_however_many_clocks_there_are() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    for photo in 0..10 {
        let name = format!("photo {photo}");
        sim.write_to(PHONE, group, set(captions(group), &name, "untitled"));
        sim.write_to(PHONE, group, set(notes(group), &name, "seen"));
    }
    sim.settle();
    let height = sim.devices[PHONE].groups[&group].folded();
    let committed = |sim: &Sim| {
        let count = |model| {
            sim.read(NAS, &Query::prefix(group, model, b"").committed())
                .len()
        };

        count(captions(group)) + count(notes(group))
    };

    sim.connect(NAS, PHONE);
    while sim.devices[NAS].groups[&group].reached() < height {
        assert!(sim.deliver(), "it never heard");
    }
    let first = committed(&sim);
    assert_eq!(first, crate::db::group::STRIDE);

    // Each step after commits as much again, and no more.
    assert!(sim.resume());
    assert_eq!(committed(&sim), first * 2);
    assert!(sim.resume());
    assert_eq!(committed(&sim), first * 3);
    while sim.resume() {}
    assert_eq!(committed(&sim), 20);
}

#[test]
fn a_snapshot_carries_every_clock() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    for photo in 0..300 {
        let name = format!("photo {photo:04}");
        sim.write_to(PHONE, group, set(captions(group), &name, "untitled"));
        sim.write_to(PHONE, group, set(notes(group), &name, "seen"));
        if photo % 100 == 0 {
            sim.settle();
        }
    }
    sim.write_to(PHONE, group, Transaction::new().drop_model(tags(group)));
    sim.settle();
    sim.compact(PHONE, group);
    sim.compact(LAPTOP, group);

    sim.connect(NAS, PHONE);
    sim.run();

    for model in [captions(group), notes(group)] {
        let query = Query::prefix(group, model, b"").committed();
        assert_eq!(sim.read(NAS, &query).len(), 300);
        assert_eq!(sim.read(NAS, &query), sim.read(PHONE, &query));
    }

    // It can vouch for the next checkpoint: its roots are the others'.
    sim.isolate(LAPTOP);
    sim.connect(NAS, PHONE);
    sim.write_to(NAS, group, set(captions(group), "late", "arrival"));
    sim.settle();

    assert_eq!(
        sim.committed(PHONE, group, captions(group), "late")
            .as_deref(),
        Some("arrival")
    );
}

/// A removed contributor's work is cut in every clock it wrote to.
#[test]
fn removal_cuts_every_clock() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    sim.write_to(NAS, group, set(captions(group), "beach", "from the nas"));
    sim.write_to(NAS, group, set(notes(group), "todo", "from the nas"));
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();

    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(sim.shown(device, group, captions(group), "beach"), None);
        assert_eq!(sim.shown(device, group, notes(group), "todo"), None);
    }
}

/// An entry that arrives late is judged, and so is whatever it could
/// have changed. What follows it in the timeline and has nothing to do
/// with it is left alone.
#[test]
fn a_late_arrival_costs_what_it_changes_not_what_follows_it() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.disconnect_all();
    sim.write(NAS, set(notes(group), "from the nas", "early"));
    sim.connect(PHONE, LAPTOP);
    for index in 0..30 {
        sim.write(PHONE, set(notes(group), &format!("note {index:02}"), "x"));
    }
    sim.run();
    let before = sim.devices[LAPTOP].groups[&group].weighed();
    assert_eq!(before, 30);

    sim.connect(NAS, LAPTOP);
    sim.run();

    assert!(
        sim.shown(LAPTOP, group, notes(group), "from the nas")
            .is_some()
    );
    assert!(sim.shown(LAPTOP, group, notes(group), "note 29").is_some());
    assert_eq!(sim.devices[LAPTOP].groups[&group].weighed(), before + 1);
}

/// Committing a drop takes along every waiting entry that writes to
/// the model, whatever keys they write.
#[test]
fn a_committed_drop_takes_what_waited_to_be_written() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(LAPTOP);
    sim.write(LAPTOP, set(notes(group), "written meanwhile", "x"));
    sim.write(PHONE, Transaction::new().drop_model(notes(group)));
    sim.settle();
    assert!(
        sim.shown(LAPTOP, group, notes(group), "written meanwhile")
            .is_some()
    );

    sim.connect_all();
    sim.run();

    assert_eq!(
        sim.shown(LAPTOP, group, notes(group), "written meanwhile"),
        None
    );
}
