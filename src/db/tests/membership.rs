use super::*;
use crate::db::group::SWEEP;

#[test]
fn a_new_contributor_writes_once_a_checkpoint_admits_it() {
    let (mut sim, group) = Sim::group(2, 1);
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);

    let early = block_on(sim.devices[LAPTOP].write(group, set(notes(group), "todo", "milk")));
    assert!(matches!(early, Err(Error::NotAContributor)));

    sim.settle();
    sim.write_to(LAPTOP, group, set(notes(group), "todo", "milk"));
    sim.settle();

    assert_eq!(
        sim.committed(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
}

#[test]
fn a_request_to_reconfigure_survives_a_restart() {
    let (mut sim, group) = Sim::group(2, 1);
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.restart(PHONE);
    sim.connect_all();
    sim.settle();

    sim.write_to(LAPTOP, group, set(notes(group), "todo", "milk"));
    sim.run();

    assert_eq!(
        sim.shown(PHONE, group, notes(group), "todo").as_deref(),
        Some("milk")
    );
}

#[test]
fn a_removed_contributor_loses_what_it_wrote_after_its_last_checkpoint() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.write_to(NAS, group, set(notes(group), "kept", "committed"));
    sim.settle();
    sim.isolate(NAS);
    sim.write_to(NAS, group, set(notes(group), "lost", "never committed"));

    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();
    sim.connect(NAS, PHONE);
    sim.connect(NAS, LAPTOP);
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(
            sim.committed(device, group, notes(group), "kept")
                .as_deref(),
            Some("committed")
        );
        assert_eq!(sim.shown(device, group, notes(group), "lost"), None);
    }
    let refused = block_on(sim.devices[NAS].write(group, set(notes(group), "more", "?")));
    assert!(matches!(refused, Err(Error::NotAContributor)));
}

#[test]
fn work_written_on_top_of_a_removed_contributors_survives() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.disconnect(NAS, PHONE);
    sim.write_to(NAS, group, set(notes(group), "from nas", "uncommitted"));
    sim.run();
    sim.write_to(
        LAPTOP,
        group,
        set(notes(group), "from laptop", "after the nas"),
    );
    sim.isolate(NAS);

    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();
    sim.settle();

    for device in [PHONE, LAPTOP] {
        assert_eq!(
            sim.committed(device, group, notes(group), "from laptop")
                .as_deref(),
            Some("after the nas")
        );
    }
}

#[test]
fn a_removed_contributor_has_no_say() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();

    sim.isolate(LAPTOP);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();

    assert_eq!(
        sim.committed(PHONE, group, notes(group), "todo"),
        None,
        "the phone and the NAS aren't a quorum of two"
    );
}

#[test]
fn a_group_can_shrink_to_one_and_carry_on() {
    let (mut sim, group) = Sim::group(2, 2);
    sim.reconfigure(LAPTOP, group, [LAPTOP]);
    sim.settle();
    sim.isolate(LAPTOP);

    sim.write_to(LAPTOP, group, set(notes(group), "todo", "milk"));
    sim.settle();

    assert_eq!(
        sim.committed(LAPTOP, group, notes(group), "todo")
            .as_deref(),
        Some("milk")
    );
}

/// Found by the randomized simulation: a contributor removed and added
/// back, with something written before either that nobody had heard.
#[test]
fn coming_back_doesnt_bring_back_what_removal_cut() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    sim.write_to(NAS, group, set(notes(group), "lost", "before removal"));

    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP, NAS]);
    sim.settle();
    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(sim.shown(device, group, notes(group), "lost"), None);
    }

    sim.write_to(NAS, group, set(notes(group), "kept", "after coming back"));
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        let kept = sim.committed(device, group, notes(group), "kept");
        assert_eq!(kept.as_deref(), Some("after coming back"));
    }
}

/// The laptop writes on top of something the friend wrote, and the
/// friend is removed before anyone else has seen either. What the
/// friend wrote is gone for good. What the laptop wrote isn't, and
/// letting the friend back in doesn't leave it waiting for an entry
/// that will never come.
#[test]
fn work_on_top_of_a_cut_entry_counts_even_once_its_author_is_back() {
    let (mut sim, group) = Sim::group(4, 4);
    let everyone = |it: usize| (it, Role::contributor(u32::from(it != FRIEND)));
    sim.assign(PHONE, group, (0..4).map(everyone));
    sim.settle();
    sim.disconnect_all();

    sim.connect(FRIEND, LAPTOP);
    write(&mut sim, FRIEND, "guest", "was here");
    sim.run();
    write(&mut sim, LAPTOP, "host", "saw that");
    sim.disconnect_all();

    // The phone and the NAS remove the friend, and let it back in.
    sim.connect(PHONE, NAS);
    sim.assign(PHONE, group, (0..3).map(everyone));
    sim.settle();
    sim.assign(PHONE, group, (0..4).map(everyone));
    sim.settle();

    sim.connect_all();
    sim.settle();

    let model = household_notes(&sim);
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let host = sim.committed(device, group, model, "host");
        assert_eq!(host.as_deref(), Some("saw that"), "device {device}");
        assert_eq!(sim.committed(device, group, model, "guest"), None);
        assert_eq!(sim.shown(device, group, model, "guest"), None);
    }
}

/// A member that asks for a change needn't be the one to propose it,
/// or be there when it's proposed.
#[test]
fn a_request_is_carried_out_by_whoever_proposes_next() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.run();
    sim.isolate(PHONE);
    // Whoever heard it holds it for as long as it stands.
    sim.restart(LAPTOP);
    sim.restart(NAS);
    sim.connect(LAPTOP, NAS);
    sim.settle();

    let written = block_on(sim.devices[NAS].write(group, set(notes(group), "todo", "milk")));
    assert!(matches!(written, Err(Error::NotAContributor)));

    // It was made of the group as it was, and is spent.
    sim.restart(LAPTOP);
    sim.connect_all();
    sim.reconfigure(LAPTOP, group, [PHONE, LAPTOP, NAS]);
    sim.settle();
    sim.settle();

    sim.write_to(NAS, group, set(notes(group), "todo", "milk"));
    sim.settle();
    assert!(sim.committed(PHONE, group, notes(group), "todo").is_some());
}

/// Only a member with a say can ask for a change.
#[test]
fn a_request_from_a_member_with_no_say_is_ignored() {
    use super::super::message::{Message, Request, Summary};
    use crate::actor::{Identity, Peer, Signed};
    use crate::chain::{Electorate, Gossip};

    let (mut sim, group) = Sim::group(3, 2);
    let everyone = [PHONE, LAPTOP, NAS].map(|it| (Peer::from(sim.id(it)), Role::contributor(1)));
    let request = Request {
        group,
        since: sim.devices[LAPTOP].groups[&group].tip().height,
        electorate: Electorate::of(everyone),
    };
    let identity = Identity::from_secret_key(secret(NAS));
    let message = Message {
        group,
        summary: Summary::default(),
        chain: Gossip::default(),
        entries: Vec::new(),
        links: Vec::new(),
        restored: Vec::new(),
        proofs: Vec::new(),
        snapshot: None,
        requests: vec![Signed::sign(&identity, request).signed().clone()],
        secret: None,
    };
    let from = sim.id(NAS);
    let received = block_on(sim.devices[LAPTOP].receive(from, &crate::encoding::encode(&message)));
    sim.absorb(LAPTOP, received.expect("received"));
    sim.settle();
    sim.settle();

    let written = block_on(sim.devices[NAS].write(group, set(notes(group), "todo", "milk")));
    assert!(matches!(written, Err(Error::NotAContributor)));
}

/// How many entries of the home group's own clock the device stores.
fn stored(sim: &Sim, device: usize, group: GroupId) -> usize {
    let view = sim.devices[device].store.view();
    let (start, end) = keys::range(keys::logs(group));

    block_on(crate::db::step::scan(&view, &start, &end))
        .expect("scanned")
        .len()
}

const MANY: usize = 30;

/// The nas writes a lot that nobody hears, and is removed.
fn removed_with_a_lot_unheard(sim: &mut Sim, group: GroupId) {
    sim.isolate(NAS);
    for note in 0..MANY {
        sim.write_to(NAS, group, set(notes(group), &format!("lost {note}"), "x"));
    }
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();
}

/// Dropping what a removed contributor wrote is decided at once, and
/// what's stored of it is deleted a few entries a step: however much
/// there is, no step does all of it.
#[test]
fn what_removal_cut_is_deleted_a_few_entries_a_step() {
    let (mut sim, group) = Sim::group(3, 3);
    removed_with_a_lot_unheard(&mut sim, group);
    let burying = |sim: &Sim| sim.devices[NAS].groups[&group].burying();
    let before = stored(&sim, NAS, group);

    sim.connect_all();
    while burying(&sim) == 0 {
        assert!(sim.deliver(), "the nas never heard");
    }
    let (mut left, mut kept) = (burying(&sim), stored(&sim, NAS, group));
    assert_eq!(left, MANY - SWEEP);
    assert_eq!(kept, before - SWEEP);

    // It carries on by itself, and through being stopped.
    sim.isolate(NAS);
    assert!(sim.devices[NAS].groups[&group].is_busy());
    sim.wake(NAS);
    let outputs = block_on(sim.devices[NAS].resume()).expect("resumed");
    sim.absorb(NAS, outputs);
    assert_eq!(burying(&sim), left - SWEEP);
    sim.restart(NAS);
    assert_eq!(burying(&sim), left - SWEEP);
    (left, kept) = (burying(&sim), stored(&sim, NAS, group));
    while sim.devices[NAS].groups[&group].is_busy() {
        sim.wake(NAS);
        let outputs = block_on(sim.devices[NAS].resume()).expect("resumed");
        sim.absorb(NAS, outputs);
        let now = (burying(&sim), stored(&sim, NAS, group));
        assert!(left - now.0 <= SWEEP && kept - now.1 <= SWEEP, "{now:?}");
        (left, kept) = now;
    }

    assert_eq!(burying(&sim), 0);
    assert_eq!(stored(&sim, NAS, group), before - MANY);
    for note in 0..MANY {
        let lost = sim.shown(NAS, group, notes(group), &format!("lost {note}"));
        assert_eq!(lost, None, "note {note}");
    }
}

/// A contributor let back in starts its chain again where it was cut,
/// which is where the entries it lost may still be stored.
#[test]
fn a_contributor_back_writes_over_what_its_still_deleting() {
    let (mut sim, group) = Sim::group(3, 3);
    removed_with_a_lot_unheard(&mut sim, group);
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP, NAS]);
    sim.settle();
    let burying = |sim: &Sim| sim.devices[NAS].groups[&group].burying();

    sim.connect_all();
    while burying(&sim) == 0 {
        assert!(sim.deliver(), "the nas never heard");
    }
    while sim.devices[NAS].groups[&group].folded() < sim.devices[PHONE].groups[&group].folded() {
        assert!(sim.deliver(), "the nas never caught up");
    }
    assert!(burying(&sim) > SWEEP, "{} left", burying(&sim));
    for note in 0..MANY {
        sim.write_to(NAS, group, set(notes(group), &format!("kept {note}"), "y"));
    }
    sim.restart(NAS);
    sim.connect_all();
    sim.settle();

    assert_eq!(burying(&sim), 0);
    for device in [PHONE, LAPTOP, NAS] {
        for note in 0..MANY {
            let kept = sim.committed(device, group, notes(group), &format!("kept {note}"));
            assert_eq!(kept.as_deref(), Some("y"), "device {device}, note {note}");
        }
        for note in 0..MANY {
            let lost = sim.shown(device, group, notes(group), &format!("lost {note}"));
            assert_eq!(lost, None, "device {device}, note {note}");
        }
    }
}

/// Carries on as [`Sim::settle`] does, but for one device, which is
/// left with whatever it had to do.
fn settle_without(sim: &mut Sim, idle: usize, group: GroupId) {
    for _ in 0..12 {
        for device in (0..sim.devices.len()).filter(|&device| device != idle) {
            sim.tick(device);
        }
        loop {
            if sim.deliver() {
                continue;
            }
            let busy = |device: &usize| sim.devices[*device].groups[&group].is_busy();
            let Some(device) = (0..sim.devices.len()).filter(|&it| it != idle).find(busy) else {
                break;
            };
            sim.wake(device);
            let outputs = block_on(sim.devices[device].resume()).expect("resumed");
            sim.absorb(device, outputs);
        }
    }
}

/// A reader hears a lot from the nas that nobody else does. The nas
/// is removed and let back in, and the reader, hearing of it, is
/// stopped partway through deleting what it heard. Meanwhile the nas
/// writes as much again, which is committed.
fn a_reader_partway_through_deleting(sim: &mut Sim, group: GroupId, each: usize) {
    sim.disconnect_all();
    sim.connect(NAS, FRIEND);
    for note in 0..each {
        sim.write_to(NAS, group, set(notes(group), &format!("lost {note}"), "x"));
    }
    sim.run();
    sim.disconnect_all();
    sim.connect(PHONE, LAPTOP);
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP, NAS]);
    sim.settle();

    sim.connect(FRIEND, PHONE);
    let folded = |sim: &Sim, device: usize| sim.devices[device].groups[&group].folded();
    while folded(sim, FRIEND) < folded(sim, PHONE) {
        assert!(sim.deliver(), "the reader never caught up");
    }
    sim.isolate(FRIEND);
    let burying = sim.devices[FRIEND].groups[&group].burying();
    assert!(burying > SWEEP, "{burying} left");

    sim.connect(NAS, PHONE);
    sim.connect(NAS, LAPTOP);
    settle_without(sim, FRIEND, group);
    for note in 0..each {
        sim.write_to(NAS, group, set(notes(group), &format!("kept {note}"), "y"));
    }
    settle_without(sim, FRIEND, group);
    assert_eq!(sim.devices[FRIEND].groups[&group].burying(), burying);
}

fn only_what_was_kept_shows(sim: &Sim, device: usize, group: GroupId, each: usize) {
    assert_eq!(sim.devices[device].groups[&group].burying(), 0);
    for note in 0..each {
        let kept = sim.committed(device, group, notes(group), &format!("kept {note}"));
        assert_eq!(kept.as_deref(), Some("y"), "note {note}");
    }
    for note in 0..each {
        let lost = sim.shown(device, group, notes(group), &format!("lost {note}"));
        assert_eq!(lost, None, "note {note}");
    }
}

/// What a contributor let back in writes arrives where the entries it
/// lost are still stored, on a device that's still deleting them.
#[test]
fn entries_arrive_over_ones_still_being_deleted() {
    let (mut sim, group) = Sim::group(4, 3);
    a_reader_partway_through_deleting(&mut sim, group, MANY);

    sim.connect(FRIEND, PHONE);
    sim.settle();
    only_what_was_kept_shows(&sim, FRIEND, group, MANY);
}

/// The same, when what was written since arrives as a snapshot: the
/// author's chain is then past all of it at once.
#[test]
fn a_snapshot_arrives_over_entries_still_being_deleted() {
    let (mut sim, group) = Sim::group(4, 3);
    a_reader_partway_through_deleting(&mut sim, group, 10 * MANY);
    sim.compact(PHONE, group);

    sim.connect(FRIEND, PHONE);
    let folded = |sim: &Sim, device: usize| sim.devices[device].groups[&group].folded();
    let mut left = 0;
    while folded(&sim, FRIEND) < folded(&sim, PHONE) {
        left = sim.devices[FRIEND].groups[&group].burying();
        assert!(sim.deliver() || sim.resume(), "the reader never caught up");
    }
    assert!(left > SWEEP, "{left} left when the snapshot was taken on");
    sim.restart(FRIEND);
    sim.connect(FRIEND, PHONE);
    sim.settle();
    only_what_was_kept_shows(&sim, FRIEND, group, 10 * MANY);
}
