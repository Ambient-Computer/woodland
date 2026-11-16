use super::*;

mod clocks;
mod install;
mod restore;

/// Fills a group's notes, so its state takes several pieces to send.
fn fill(sim: &mut Sim, group: GroupId, notes_written: usize) {
    for note in 0..notes_written {
        sim.write_to(
            PHONE,
            group,
            set(notes(group), &format!("note {note:04}"), "written"),
        );
        if note % 100 == 0 {
            sim.settle();
        }
    }
    sim.settle();
}

fn all_notes(sim: &Sim, device: usize, group: GroupId, committed: bool) -> Vec<(Vec<u8>, Vec<u8>)> {
    let query = Query::prefix(group, notes(group), b"");
    let query = if committed { query.committed() } else { query };

    sim.read(device, &query)
}

#[test]
fn compacting_sheds_history_and_changes_no_read() {
    let (mut sim, group) = Sim::group(2, 2);
    fill(&mut sim, group, 20);
    sim.write_to(LAPTOP, group, set(notes(group), "pending", "uncommitted"));
    sim.run();
    let before = (
        all_notes(&sim, PHONE, group, false),
        all_notes(&sim, PHONE, group, true),
    );
    assert_eq!(sim.entries_kept(PHONE, group), 21);

    sim.compact(PHONE, group);

    assert_eq!(
        sim.entries_kept(PHONE, group),
        1,
        "only what isn't committed"
    );
    let after = (
        all_notes(&sim, PHONE, group, false),
        all_notes(&sim, PHONE, group, true),
    );
    assert_eq!(after, before);

    sim.settle();
    sim.restart(PHONE);

    assert_eq!(
        sim.committed(PHONE, group, notes(group), "pending")
            .as_deref(),
        Some("uncommitted")
    );
    assert_eq!(all_notes(&sim, PHONE, group, true).len(), 21);
}

#[test]
fn a_committed_removal_leaves_nothing_behind() {
    let (mut sim, group) = Sim::group(1, 1);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();
    sim.write_to(
        PHONE,
        group,
        Transaction::new().remove(notes(group), "todo"),
    );
    sim.settle();
    sim.compact(PHONE, group);

    let (start, end) = keys::range(keys::group(group));
    let view = sim.devices[PHONE].store.view();
    let stored = block_on(crate::db::step::scan(&view, &start, &end)).expect("scanned");
    let data = stored
        .iter()
        .filter(|(key, _)| !key.starts_with(&keys::protocol(group)));

    assert_eq!(data.count(), 0, "no value, no tombstone");
    assert_eq!(sim.entries_kept(PHONE, group), 0);
}

#[test]
fn a_device_far_behind_catches_up_from_a_snapshot() {
    let (mut sim, group) = Sim::group(3, 2);
    sim.isolate(NAS);
    fill(&mut sim, group, 600);
    sim.write_to(PHONE, group, Transaction::new().drop_model(names(group)));
    sim.settle();
    sim.compact(PHONE, group);
    sim.compact(LAPTOP, group);
    sim.write_to(LAPTOP, group, set(notes(group), "pending", "uncommitted"));
    sim.run();

    sim.connect(NAS, PHONE);
    sim.run();

    assert_eq!(all_notes(&sim, NAS, group, true).len(), 600);
    for committed in [true, false] {
        assert_eq!(
            all_notes(&sim, NAS, group, committed),
            all_notes(&sim, PHONE, group, committed)
        );
    }
    assert_eq!(
        sim.shown(NAS, group, notes(group), "pending").as_deref(),
        Some("uncommitted")
    );
    assert_eq!(sim.entries_kept(NAS, group), 1);

    // It carries on from there like anyone else, dropped models included.
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP, NAS]);
    sim.settle();
    sim.write_to(NAS, group, claim(names(group), "alice", "nas"));
    sim.write_to(NAS, group, set(notes(group), "from nas", "hello"));
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.committed(device, group, names(group), "alice"),
            None,
            "the model was dropped"
        );
        assert_eq!(
            sim.committed(device, group, notes(group), "from nas")
                .as_deref(),
            Some("hello")
        );
    }
}

#[test]
fn a_snapshot_that_isnt_the_committed_state_is_refused() {
    let (mut sim, group) = Sim::group(2, 1);
    sim.isolate(LAPTOP);
    fill(&mut sim, group, 300);
    sim.compact(PHONE, group);

    // The phone's copy is wrong, and it doesn't know.
    let forged = Write::Put {
        key: keys::committed_value(group, notes(group), b"note 0007"),
        value: b"forged".to_vec(),
    };
    block_on(sim.devices[PHONE].store.apply(vec![forged])).expect("applied");
    sim.connect(PHONE, LAPTOP);
    sim.run();

    assert_eq!(all_notes(&sim, LAPTOP, group, true), []);
    let (start, end) = keys::range(keys::staged(group));
    let view = sim.devices[LAPTOP].store.view();
    let staged = block_on(crate::db::step::scan(&view, &start, &end)).expect("scanned");
    assert_eq!(staged, [], "nothing of it is kept");
}

#[test]
fn work_written_while_away_lands_on_top_of_a_snapshot() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    sim.write_to(NAS, group, claim(names(group), "alice", "nas"));
    sim.write_to(NAS, group, set(notes(group), "from nas", "hello"));
    fill(&mut sim, group, 300);
    sim.write_to(PHONE, group, claim(names(group), "alice", "phone"));
    sim.settle();
    sim.compact(PHONE, group);
    sim.compact(LAPTOP, group);

    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.committed(device, group, names(group), "alice")
                .as_deref(),
            Some("phone")
        );
        assert_eq!(
            sim.committed(device, group, notes(group), "from nas")
                .as_deref(),
            Some("hello")
        );
        assert_eq!(all_notes(&sim, device, group, true).len(), 301);
    }
}

#[test]
fn a_contributor_removed_while_away_learns_it_from_a_snapshot() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    sim.write_to(NAS, group, set(notes(group), "lost", "never committed"));
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();
    fill(&mut sim, group, 10);
    sim.compact(PHONE, group);
    sim.compact(LAPTOP, group);

    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(sim.shown(device, group, notes(group), "lost"), None);
        assert_eq!(all_notes(&sim, device, group, true).len(), 10);
    }
    let refused = block_on(sim.devices[NAS].write(group, set(notes(group), "more", "?")));
    assert!(matches!(refused, Err(Error::NotAContributor)));
}

/// A newcomer is handed only the checkpoints that changed who decides,
/// and the latest. That's enough to check the latest for itself.
#[test]
fn a_newcomer_follows_the_changes_of_electorate_through_a_shed_chain() {
    let mut sim = Sim::new(4);
    let (group, outputs) = block_on(sim.devices[PHONE].found([7; 32])).expect("founded");
    sim.absorb(PHONE, outputs);
    let invitation = sim.devices[PHONE].invite(group).expect("held");
    for device in [LAPTOP, NAS] {
        let (_, outputs) = block_on(sim.devices[device].join(&invitation)).expect("joined");
        sim.absorb(device, outputs);
    }
    sim.connect(PHONE, LAPTOP);
    sim.connect(PHONE, NAS);
    sim.connect(LAPTOP, NAS);

    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();
    fill(&mut sim, group, 5);
    sim.reconfigure(LAPTOP, group, [LAPTOP, NAS]);
    sim.settle();
    sim.write_to(NAS, group, set(notes(group), "from nas", "hello"));
    sim.settle();
    for device in [PHONE, LAPTOP, NAS] {
        sim.compact(device, group);
    }

    let (_, outputs) = block_on(sim.devices[FRIEND].join(&invitation)).expect("joined");
    sim.absorb(FRIEND, outputs);
    sim.connect(FRIEND, NAS);
    sim.run();

    assert_eq!(
        all_notes(&sim, FRIEND, group, true),
        all_notes(&sim, NAS, group, true)
    );
    assert_eq!(all_notes(&sim, FRIEND, group, true).len(), 6);
}

#[test]
fn a_snapshot_carries_on_after_a_restart() {
    let (mut sim, group) = Sim::group(2, 1);
    sim.isolate(LAPTOP);
    fill(&mut sim, group, 700);
    sim.compact(PHONE, group);
    sim.connect(PHONE, LAPTOP);
    let (start, end) = keys::range(keys::staged(group));
    let staged = |sim: &Sim| {
        let view = sim.devices[LAPTOP].store.view();

        block_on(crate::db::step::scan(&view, &start, &end)).expect("scanned")
    };
    for _ in 0..8 {
        if !staged(&sim).is_empty() {
            break;
        }
        sim.deliver();
    }
    assert!(
        !staged(&sim).is_empty(),
        "the test means to stop part of the way through"
    );
    assert_eq!(
        all_notes(&sim, LAPTOP, group, true),
        [],
        "nothing is read before it's checked"
    );

    sim.restart(LAPTOP);
    sim.connect(PHONE, LAPTOP);
    sim.run();

    assert_eq!(all_notes(&sim, LAPTOP, group, true).len(), 700);
}

#[test]
fn an_archive_serves_history_to_a_peer_that_wants_it() {
    let (mut sim, group) = Sim::group(3, 2);
    sim.isolate(NAS);
    fill(&mut sim, group, 30);
    sim.compact(LAPTOP, group);

    // The phone never compacted, so the NAS gets every entry from it.
    sim.connect(NAS, PHONE);
    sim.run();

    assert_eq!(sim.entries_kept(NAS, group), 30);
    assert_eq!(all_notes(&sim, NAS, group, true).len(), 30);
}

/// What's shed is as good as gone at once, and deleted so much a step.
#[test]
fn shed_history_is_deleted_a_few_entries_at_a_time() {
    let (mut sim, group) = Sim::group(3, 2);
    sim.isolate(NAS);
    fill(&mut sim, group, 20);
    let elsewhere = ModelId::kv(ClockId::new(group, [7; 32]), [1; 32]);
    for index in 0..20 {
        sim.write_to(PHONE, group, set(elsewhere, &format!("{index}"), "x"));
    }
    sim.settle();
    let before = all_notes(&sim, PHONE, group, true);

    // So many a step, however many clocks they're in.
    let outputs = block_on(sim.devices[PHONE].compact(group)).expect("compacted");
    assert!(outputs.unfinished);
    sim.absorb(PHONE, outputs);
    let left = sim.entries_kept(PHONE, group);
    assert_eq!(left, 40 - crate::db::group::SWEEP);
    assert_eq!(all_notes(&sim, PHONE, group, true), before);

    // It carries on from its store alone, the same amount each step.
    sim.restart(PHONE);
    assert!(sim.resume());
    let fewer = sim.entries_kept(PHONE, group);
    assert!(fewer < left);
    assert!(sim.resume());
    let remaining = sim.entries_kept(PHONE, group);
    assert_eq!(remaining, fewer - (left - fewer));
    assert!(remaining > 0);

    // A peer that's behind is sent the state, not what's left of the
    // history.
    sim.connect(NAS, PHONE);
    while sim.deliver() || sim.devices[NAS].groups[&group].is_busy() {
        let outputs = block_on(sim.devices[NAS].resume()).expect("resumed");
        sim.absorb(NAS, outputs);
    }
    assert_eq!(all_notes(&sim, NAS, group, true), before);
    assert_eq!(sim.entries_kept(NAS, group), 0);
    while sim.resume() {}

    assert_eq!(sim.entries_kept(PHONE, group), 0);
    assert_eq!(all_notes(&sim, PHONE, group, true), before);
}

/// How many entries the device stores.
fn entries_stored(sim: &Sim, device: usize, group: GroupId) -> usize {
    let view = sim.devices[device].store.view();
    let (start, end) = keys::range(keys::logs(group));

    block_on(crate::db::step::scan(&view, &start, &end))
        .expect("scanned")
        .len()
}

/// A device that's behind may take what's committed instead of the
/// history that led to it, though the history is there to be had. It
/// ends up with the same state. Once it has caught up, it's sent
/// transactions again.
#[test]
fn a_device_may_take_state_though_history_is_to_be_had() {
    const BEHIND: usize = 20;
    let behind = |skip: bool| {
        let (mut sim, group) = Sim::group(3, 2);
        sim.isolate(NAS);
        for note in 0..BEHIND {
            sim.write(PHONE, set(notes(group), &format!("note {note}"), "x"));
            sim.settle();
        }
        if skip {
            let outputs = block_on(sim.devices[NAS].skip_ahead(group)).expect("asked");
            sim.absorb(NAS, outputs);
        }
        sim.connect_all();
        sim.settle();

        (sim, group)
    };
    let all = |sim: &Sim, group: GroupId| {
        sim.read(NAS, &Query::prefix(group, notes(group), b"").committed())
    };

    let (mut sim, group) = behind(true);
    let (slow, _) = behind(false);
    assert_eq!(all(&sim, group).len(), BEHIND);
    assert_eq!(all(&sim, group), all(&slow, group));
    assert_eq!(entries_stored(&slow, NAS, group), BEHIND);
    assert_eq!(entries_stored(&sim, NAS, group), 0);
    let folded = |sim: &Sim, device: usize| sim.devices[device].groups[&group].folded();
    assert_eq!(folded(&sim, NAS), folded(&sim, PHONE));

    // It asked once. From here on it's sent what's written.
    sim.restart(NAS);
    sim.connect_all();
    sim.write(PHONE, set(notes(group), "after", "y"));
    sim.settle();
    assert_eq!(all(&sim, group).len(), BEHIND + 1);
    assert_eq!(entries_stored(&sim, NAS, group), 1);
}

/// Asking for state when there's nothing to catch up on changes
/// nothing, until there is.
#[test]
fn asking_for_state_waits_until_theres_some_to_take() {
    let (mut sim, group) = Sim::group(3, 2);
    let outputs = block_on(sim.devices[NAS].skip_ahead(group)).expect("asked");
    sim.absorb(NAS, outputs);
    sim.settle();
    sim.restart(NAS);

    for note in 0..3 {
        sim.write(PHONE, set(notes(group), &format!("note {note}"), "x"));
    }
    sim.settle();
    sim.connect_all();
    sim.settle();
    let all = sim.read(NAS, &Query::prefix(group, notes(group), b"").committed());
    assert_eq!(all.len(), 3);
    assert_eq!(entries_stored(&sim, NAS, group), 0);
}
