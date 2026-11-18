use std::collections::BTreeSet;

use super::*;
use crate::db::group::STRIDE;

#[test]
fn nothing_commits_without_a_quorum() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(PHONE);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();

    assert_eq!(
        sim.shown(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
    assert_eq!(sim.committed(PHONE, group, notes(group), "todo"), None);

    sim.isolate(NAS);
    sim.connect(PHONE, LAPTOP);
    sim.settle();

    assert_eq!(
        sim.committed(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
    assert_eq!(
        sim.committed(LAPTOP, group, notes(group), "todo")
            .as_deref(),
        Some("milk")
    );
    assert_eq!(sim.committed(NAS, group, notes(group), "todo"), None);
}

#[test]
fn a_quorum_commits_while_a_contributor_is_away() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();

    assert_eq!(
        sim.committed(LAPTOP, group, notes(group), "todo")
            .as_deref(),
        Some("milk")
    );

    sim.connect(NAS, LAPTOP);
    sim.run();

    assert_eq!(
        sim.committed(NAS, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
}

#[test]
fn committed_work_isnt_undone_by_work_that_arrives_late() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);

    // The NAS claims first, by any clock. Nobody hears.
    sim.write_to(NAS, group, claim(names(group), "alice", "nas"));
    for round in 0..5 {
        sim.write_to(PHONE, group, set(notes(group), "busy", &round.to_string()));
    }
    sim.write_to(PHONE, group, claim(names(group), "alice", "phone"));
    sim.settle();

    assert_eq!(
        sim.committed(LAPTOP, group, names(group), "alice")
            .as_deref(),
        Some("phone")
    );
    assert_eq!(
        sim.shown(NAS, group, names(group), "alice").as_deref(),
        Some("nas")
    );

    sim.connect(NAS, PHONE);
    sim.connect(NAS, LAPTOP);
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.shown(device, group, names(group), "alice").as_deref(),
            Some("phone")
        );
        assert_eq!(
            sim.committed(device, group, names(group), "alice")
                .as_deref(),
            Some("phone")
        );
    }
}

#[test]
fn late_work_that_conflicts_with_nothing_still_happens() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    sim.write_to(NAS, group, set(notes(group), "from nas", "hello"));
    sim.write_to(PHONE, group, set(notes(group), "from phone", "hi"));
    sim.settle();

    sim.connect(NAS, PHONE);
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.committed(device, group, notes(group), "from nas")
                .as_deref(),
            Some("hello")
        );
        assert_eq!(
            sim.committed(device, group, notes(group), "from phone")
                .as_deref(),
            Some("hi")
        );
    }
}

#[test]
fn a_device_restarted_in_the_middle_of_an_attempt_still_agrees() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.run();
    sim.tick(PHONE);
    for _ in 0..3 {
        sim.deliver();
    }

    sim.restart(LAPTOP);
    sim.restart(PHONE);
    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.committed(device, group, notes(group), "todo")
                .as_deref(),
            Some("milk")
        );
    }
}

#[test]
fn rival_attempts_end_in_one_checkpoint() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.disconnect(PHONE, LAPTOP);
    sim.write_to(PHONE, group, claim(names(group), "alice", "phone"));
    sim.write_to(LAPTOP, group, claim(names(group), "alice", "laptop"));
    sim.tick(PHONE);
    sim.tick(LAPTOP);
    sim.connect(PHONE, LAPTOP);
    sim.settle();

    let holder = sim.committed(NAS, group, names(group), "alice");
    assert!(holder.is_some());
    for device in [PHONE, LAPTOP] {
        assert_eq!(sim.committed(device, group, names(group), "alice"), holder);
    }
}

/// A threshold model shows only what's committed, so nobody acts on a
/// value that could still be reordered.
#[test]
fn a_threshold_value_shows_once_its_committed() {
    let (mut sim, group) = Sim::group(3, 3);
    let limits = ModelId::threshold(group, [4; 32]);
    sim.isolate(PHONE);
    sim.write_to(PHONE, group, set(limits, "quota", "10"));
    sim.settle();

    assert_eq!(sim.shown(PHONE, group, limits, "quota"), None);

    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.shown(device, group, limits, "quota").as_deref(),
            Some("10")
        );
    }
}

/// A vote says "I reached this state too". A contributor that didn't,
/// for whatever reason, doesn't vote, and without it there's no quorum.
#[test]
fn a_contributor_that_reaches_another_state_doesnt_vote() {
    let (mut sim, group) = Sim::group(2, 2);
    let corrupt = Write::Put {
        key: keys::state(group, group.into()),
        value: vec![7; 4096],
    };
    block_on(sim.devices[LAPTOP].store.apply(vec![corrupt])).expect("applied");

    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();

    assert_eq!(
        sim.shown(LAPTOP, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
    for device in [PHONE, LAPTOP] {
        assert_eq!(sim.committed(device, group, notes(group), "todo"), None);
    }
}

/// A checkpoint can cover any amount of work, and a step does only so
/// much. The rest is done by the steps after, whatever happens in
/// between, and no reader sees any of it until it's all there.
#[test]
fn a_big_checkpoint_is_committed_a_few_entries_at_a_time() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    for index in 0..20 {
        sim.write(PHONE, set(notes(group), &format!("note {index:02}"), "x"));
    }
    sim.settle();
    let height = sim.devices[PHONE].groups[&group].folded();
    let note = |sim: &Sim, index: usize| {
        let key = format!("note {index:02}");

        sim.committed(NAS, group, notes(group), &key)
    };

    // The NAS hears of all of it at once, and makes a start.
    sim.connect(NAS, PHONE);
    while sim.devices[NAS].groups[&group].reached() < height {
        assert!(sim.deliver(), "it never heard");
    }
    assert!(sim.devices[NAS].groups[&group].folded() < height);
    let everything = Query::prefix(group, notes(group), b"").committed();
    let stored = sim.stored(NAS, &everything).len();
    assert!((1..20).contains(&stored), "some of it is in place");
    assert_eq!(sim.read(NAS, &everything), [], "and none of it shows");

    // Nothing stands in for history until all of it is committed.
    sim.compact(NAS, group);
    assert_eq!(sim.entries_kept(NAS, group), 20);

    // It carries on from where it was, from its store alone.
    sim.restart(NAS);
    let resumed = block_on(sim.devices[NAS].resume()).expect("resumed");
    assert!(resumed.unfinished);
    assert!(sim.stored(NAS, &everything).len() > stored);
    assert_eq!(sim.read(NAS, &everything), []);
    while sim.resume() {
        let shown = sim.read(NAS, &everything).len();
        assert!(shown == 0 || shown == 20, "all of it or none");
    }

    assert_eq!(sim.devices[NAS].groups[&group].folded(), height);
    assert!((0..20).all(|index| note(&sim, index).is_some()));
    sim.compact(NAS, group);
    assert_eq!(sim.entries_kept(NAS, group), 0);
}

/// A device with a say commits what a checkpoint covers for itself
/// before it vouches for it, to one side, and that's as much work as
/// committing it.
#[test]
fn what_a_checkpoint_comes_to_is_worked_out_a_few_entries_at_a_time() {
    let (mut sim, group) = Sim::group(3, 3);
    for index in 0..20 {
        sim.write(PHONE, set(notes(group), &format!("note {index:02}"), "x"));
    }
    sim.run();
    sim.isolate(NAS);
    let height = sim.devices[PHONE].groups[&group].folded();

    // Proposing it waits for that, and nothing shows meanwhile.
    sim.tick(PHONE);
    let mut steps = 0;
    while !sim.devices[PHONE].groups[&group].attempting() {
        assert!(sim.kept_aside(PHONE, group) > 0);
        assert_eq!(sim.committed(PHONE, group, notes(group), "note 00"), None);
        assert!(sim.resume(), "it gave up");
        steps += 1;
    }
    assert!(steps >= 20 / STRIDE);

    // So does voting for it.
    while sim.devices[LAPTOP].groups[&group].proposals() == 0 {
        assert!(sim.deliver(), "it never heard");
    }
    assert!(sim.devices[LAPTOP].groups[&group].is_busy());
    assert_eq!(sim.devices[PHONE].groups[&group].folded(), height);

    // A device that stops in the middle starts again.
    while sim.kept_aside(LAPTOP, group) < 2 * STRIDE {
        sim.wake(LAPTOP);
        let outputs = block_on(sim.devices[LAPTOP].resume()).expect("resumed");
        sim.absorb(LAPTOP, outputs);
    }
    sim.restart(LAPTOP);
    while sim.resume() {}
    assert_eq!(sim.kept_aside(LAPTOP, group), 0);
    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert!(sim.devices[device].groups[&group].folded() > height);
        assert!(
            sim.committed(device, group, notes(group), "note 19")
                .is_some()
        );
        // Nothing of the working out is left behind.
        assert_eq!(sim.kept_aside(device, group), 0);
    }
}

/// What a device has worked out is what one checkpoint comes to. Asked
/// about another, it starts again, with nothing left of the first.
#[test]
fn working_out_one_checkpoint_says_nothing_of_another() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.disconnect_all();
    for index in 0..10 {
        sim.write(PHONE, set(notes(group), &format!("note {index:02}"), "x"));
    }
    let mut step = crate::db::step::Step::new(sim.devices[PHONE].store.view());
    let phone = sim.devices[PHONE].groups.get_mut(&group).expect("held");
    let mut ask = |all: bool, times: usize| {
        let mut found = None;
        for _ in 0..times {
            found = block_on(phone.foresee(all, &mut step)).expect("read");
            if found.is_some() {
                break;
            }
        }

        found
    };

    let as_it_is = ask(false, 1).expect("nothing to work out");
    assert_eq!(ask(true, 1), None);
    assert_eq!(ask(false, 100).as_ref(), Some(&as_it_is));

    let with_it_all = ask(true, 100).expect("worked out");
    assert_ne!(with_it_all, as_it_is);
    assert_eq!(ask(false, 100), Some(as_it_is));
    assert_eq!(ask(true, 100), Some(with_it_all));
}

/// Rivals that keep outranking each other decide nothing. A device
/// whose attempts go nowhere waits longer before each.
#[test]
fn a_device_waits_longer_before_each_attempt_at_the_same_checkpoint() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(PHONE);
    sim.write(PHONE, set(notes(group), "todo", "milk"));

    let mut waits = Vec::new();
    let (mut waited, mut last) = (0, None);
    while waits.len() < 4 {
        sim.tick(PHONE);
        while sim.resume() {}
        waited += 1;
        let attempt = sim.devices[PHONE].groups[&group].attempt();
        if attempt != last {
            waits.push(waited);
            (waited, last) = (0, attempt);
        }
        assert!(waited < 1000, "it gave up");
    }

    assert!(waits[1] < waits[2] && waits[2] < waits[3], "{waits:?}");
}

/// Any kind of model can be restricted to committed reads, and is then
/// a model of its own.
#[test]
fn any_kind_of_model_can_show_only_whats_committed() {
    let (mut sim, group) = Sim::group(3, 3);
    let open = ModelId::claims(group, [5; 32]);
    let held = open.committed_only();
    assert_ne!(open, held);
    sim.isolate(PHONE);
    for names in [open, held] {
        let claim = Transaction::new().claim(names, "ada", "phone");
        sim.write_to(PHONE, group, claim);
    }
    sim.settle();

    assert!(sim.shown(PHONE, group, open, "ada").is_some());
    assert_eq!(sim.shown(PHONE, group, held, "ada"), None);

    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.shown(device, group, held, "ada").as_deref(),
            Some("phone")
        );
    }
}

/// What's stored of the chain: one value for where it began and
/// what's undecided, and one for each decided checkpoint.
fn chain_stored(sim: &Sim, device: usize, group: GroupId) -> (usize, Vec<Vec<u8>>) {
    let view = sim.devices[device].store.view();
    let head = block_on(crate::db::step::scan(
        &view,
        &keys::chain(group),
        &keys::after(&keys::chain(group)),
    ));
    let (start, end) = keys::range(keys::links(group));
    let links = block_on(crate::db::step::scan(&view, &start, &end)).expect("scanned");
    let head = head
        .expect("scanned")
        .first()
        .map_or(0, |(_, value)| value.len());

    (head, links.into_iter().map(|(key, _)| key).collect())
}

/// Deciding a checkpoint doesn't mean writing every earlier one
/// again: each is kept by itself, and those shed are deleted.
#[test]
fn a_decided_checkpoint_is_stored_by_itself() {
    let (mut sim, group) = Sim::group(3, 3);
    let write = |sim: &mut Sim, round: usize| {
        sim.write(PHONE, set(notes(group), "round", &round.to_string()));
        sim.settle();
    };
    write(&mut sim, 0);
    let (head, few) = chain_stored(&sim, PHONE, group);
    for round in 1..12 {
        write(&mut sim, round);
    }
    let (later, many) = chain_stored(&sim, PHONE, group);
    let folded = sim.devices[PHONE].groups[&group].folded();

    assert_eq!(many.len(), few.len() + 11);
    assert_eq!(many.len(), usize::try_from(folded).expect("a few"));
    assert!(later < head * 2, "{head} grew to {later}");
    let puts = &sim.devices[PHONE].store.puts;
    assert!(many.iter().all(|key| puts[key] == 1), "written again");

    // A restart finds them all, and carries on.
    sim.restart(PHONE);
    sim.connect_all();
    write(&mut sim, 12);
    assert_eq!(sim.devices[PHONE].groups[&group].folded(), folded + 1);
    let round = sim.committed(LAPTOP, group, notes(group), "round");
    assert_eq!(round.as_deref(), Some("12"));

    // Shedding them deletes them.
    sim.compact(PHONE, group);
    let (_, kept) = chain_stored(&sim, PHONE, group);
    assert!(kept.len() < many.len(), "{} of {}", kept.len(), many.len());
    sim.restart(PHONE);
    sim.connect_all();
    write(&mut sim, 13);
    let round = sim.committed(PHONE, group, notes(group), "round");
    assert_eq!(round.as_deref(), Some("13"));
}

/// An answer says how sure it is, so that what's committed is never
/// taken for what might not be, or the other way about.
#[test]
fn a_read_says_which_level_it_answers_at() {
    let (mut sim, group) = Sim::group(2, 2);
    sim.write(PHONE, set(notes(group), "todo", "milk"));
    sim.settle();
    sim.disconnect_all();
    sim.write(PHONE, set(notes(group), "todo", "eggs"));
    let read = |query: &Query| block_on(sim.devices[PHONE].read(query)).expect("read");
    let found = |value: &str| vec![(b"todo".to_vec(), value.as_bytes().to_vec())];

    let query = Query::key(group, notes(group), b"todo");
    assert_eq!(read(&query), Answer::Optimistic(found("eggs")));
    assert_eq!(read(&query).committed(), None);
    let committed = read(&query.committed());
    assert!(committed.reached().is_some());
    assert_eq!(committed.committed(), Some(found("milk")));

    // A model that shows only what's committed answers that way
    // without being asked.
    let held = notes(group).committed_only();
    let query = Query::key(group, held, b"todo");
    assert!(read(&query).is_committed());
}

/// A device that isolated notes land on all at once: `count` of them,
/// each written `value`, in one checkpoint. Returns its height.
fn notes_for_the_nas(sim: &mut Sim, group: GroupId, count: usize, value: &str) -> u64 {
    for index in 0..count {
        sim.write(PHONE, set(notes(group), &format!("note {index:02}"), value));
    }
    sim.settle();

    sim.devices[PHONE].groups[&group].folded()
}

/// While a checkpoint is put in place, a device shows what it showed
/// before, and whatever it writes or hears of meanwhile.
#[test]
fn a_device_shows_what_it_did_while_a_checkpoint_goes_in() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    let height = notes_for_the_nas(&mut sim, group, 20, "x");
    let everything = Query::prefix(group, notes(group), b"");

    sim.connect(NAS, PHONE);
    while sim.devices[NAS].groups[&group].reached() < height {
        assert!(sim.deliver(), "it never heard");
    }
    assert!(sim.devices[NAS].groups[&group].folded() < height);

    // It holds every entry, and showed them all before any was
    // committed. It still does.
    assert_eq!(sim.read(NAS, &everything).len(), 20);
    assert_eq!(sim.read(NAS, &everything.clone().committed()), []);

    // What it writes now shows at once.
    sim.isolate(NAS);
    sim.write(NAS, set(notes(group), "mine", "y"));
    assert_eq!(sim.read(NAS, &everything).len(), 21);
    while sim.resume() {
        assert_eq!(sim.read(NAS, &everything).len(), 21);
        let committed = sim.read(NAS, &everything.clone().committed()).len();
        assert!(committed == 0 || committed == 20);
    }

    assert_eq!(sim.read(NAS, &everything.committed()).len(), 20);
}

/// A committed read says which checkpoint it's of, and that's the one
/// it shows: not one the device has finished behind the scenes while
/// starting on the next.
#[test]
fn a_committed_read_is_of_the_checkpoint_it_says() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    let base = sim.devices[NAS].groups[&group].folded();
    let first = notes_for_the_nas(&mut sim, group, 2, "first");
    let second = notes_for_the_nas(&mut sim, group, 20, "second");
    let third = notes_for_the_nas(&mut sim, group, 20, "third");
    assert!(base < first && first < second && second < third);

    // Whatever it shows at each height is all of that checkpoint.
    let expected = |height: u64| match height {
        height if height < first => vec![],
        height if height < second => vec!["first"; 2],
        height if height < third => vec!["second"; 20],
        _ => vec!["third"; 20],
    };
    let everything = Query::prefix(group, notes(group), b"").committed();
    let check = |sim: &Sim| {
        let answer = block_on(sim.devices[NAS].read(&everything)).expect("read");
        let at = answer.reached().expect("committed").checkpoint;
        let found = answer.into_found();
        let values: Vec<_> = found.iter().map(|(_, value)| value.as_slice()).collect();
        let expected: Vec<_> = expected(at).into_iter().map(str::as_bytes).collect();
        assert_eq!(values, expected, "at height {at}");

        at
    };

    sim.connect(NAS, PHONE);
    let mut seen = BTreeSet::new();
    while sim.deliver() || sim.resume() {
        seen.insert(check(&sim));
    }

    assert_eq!(check(&sim), third);
    assert!(seen.len() > 1);
}

/// A checkpoint that's ready while the last one's leftovers are still
/// being cleared away waits for that. What was kept for one is never
/// taken for what the next replaced.
#[test]
fn the_next_checkpoint_waits_for_the_last_to_be_cleared_away() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    notes_for_the_nas(&mut sim, group, 60, "first");
    let second = notes_for_the_nas(&mut sim, group, 60, "second");
    let everything = Query::prefix(group, notes(group), b"").committed();

    // The second has just begun to show, and there's a lot to clear.
    sim.connect(NAS, PHONE);
    while sim.devices[NAS].groups[&group].committed_at().checkpoint < second {
        assert!(sim.deliver() || sim.resume(), "it never got there");
    }
    assert!(sim.devices[NAS].groups[&group].is_busy());

    // The others agree on a third without it, and it hears of that in
    // the middle of the clearing. Every step of the way, a committed
    // read is of one checkpoint: see `absorb`.
    sim.isolate(NAS);
    for index in 0..60 {
        sim.write(
            PHONE,
            set(notes(group), &format!("note {index:02}"), "third"),
        );
    }
    // Whichever device comes first is the one that carries on, and
    // these two come first.
    let busy = |sim: &Sim, device: usize| sim.devices[device].groups[&group].is_busy();
    for _ in 0..12 {
        while sim.deliver() || ((busy(&sim, PHONE) || busy(&sim, LAPTOP)) && sim.resume()) {}
        sim.tick(PHONE);
        sim.tick(LAPTOP);
    }
    let third = sim.devices[PHONE].groups[&group].folded();
    assert!(third > second);
    assert!(sim.devices[NAS].groups[&group].is_busy());
    sim.connect(NAS, PHONE);
    sim.settle();

    assert_eq!(sim.devices[NAS].groups[&group].folded(), third);
    let found = sim.read(NAS, &everything);
    assert_eq!(found.len(), 60);
    assert!(found.iter().all(|(_, value)| value == b"third"));
}
