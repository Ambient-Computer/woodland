//! When a checkpoint is attempted: because the application asked, or
//! because time passed.

use super::*;

/// Delivers everything, with whoever receives a message asking for a
/// checkpoint straight after. No time passes. Returns how many
/// messages it took.
fn run_asking(sim: &mut Sim, group: GroupId) -> usize {
    let mut delivered = 0;
    loop {
        let Some(&(_, to, _)) = sim.queue.front() else {
            if sim.resume() {
                continue;
            }

            return delivered;
        };
        sim.deliver();
        sim.checkpoint(to, group);
        delivered += 1;
        assert!(delivered < 2_000, "the devices never stop talking");
    }
}

#[test]
fn a_device_alone_commits_when_asked() {
    let (mut sim, group) = Sim::group(1, 1);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    assert_eq!(sim.committed(PHONE, group, notes(group), "todo"), None);

    sim.checkpoint(PHONE, group);
    sim.run();

    let todo = sim.committed(PHONE, group, notes(group), "todo");
    assert_eq!(todo.as_deref(), Some("milk"));
}

/// Asking after every message is how to commit at the speed of the
/// network. Nobody waits for a timer, and the talking stops.
#[test]
fn asking_after_every_message_commits_without_time_passing() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.write_to(LAPTOP, group, set(notes(group), "todo", "milk"));
    sim.checkpoint(LAPTOP, group);
    run_asking(&mut sim, group);

    for device in [PHONE, LAPTOP, NAS] {
        let todo = sim.committed(device, group, notes(group), "todo");
        assert_eq!(todo.as_deref(), Some("milk"), "device {device}");
    }
}

/// Everyone asks at once, so there are rival attempts. One of them
/// is let through.
#[test]
fn rivals_that_all_ask_at_once_still_decide() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.run();
    for device in [PHONE, LAPTOP, NAS] {
        sim.checkpoint(device, group);
    }
    run_asking(&mut sim, group);
    // Whoever was outranked tries again when time has run out on the
    // attempt that outranked it, and not before.
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        let todo = sim.committed(device, group, notes(group), "todo");
        assert_eq!(todo.as_deref(), Some("milk"), "device {device}");
    }
}

#[test]
fn a_removal_is_carried_when_asked_for_after_every_message() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.checkpoint(PHONE, group);
    run_asking(&mut sim, group);

    let carried = (group, Reconfigured::Carried);
    assert!(sim.reconfigured[PHONE].contains(&carried));
    assert!(!sim.devices[LAPTOP].groups[&group].has(sim.id(NAS).into()));
}

/// Asking is not time passing: an attempt someone else has under way
/// is left to it, however often this device asks.
#[test]
fn asking_doesnt_outbid_an_attempt_under_way() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.run();
    sim.checkpoint(PHONE, group);
    // The laptop hears of the attempt and nothing more.
    assert!(sim.deliver());
    let before = sim.queue.len();
    for _ in 0..50 {
        sim.checkpoint(LAPTOP, group);
    }

    assert_eq!(sim.queue.len(), before);
    run_asking(&mut sim, group);
    let todo = sim.committed(LAPTOP, group, notes(group), "todo");
    assert_eq!(todo.as_deref(), Some("milk"));
}

/// Time told by what arrives isn't time. A device that ticks for
/// every message repeats itself to peers that do the same.
#[test]
fn ticking_for_every_message_says_more_than_it_hears() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.write_to(LAPTOP, group, set(notes(group), "todo", "milk"));
    for _ in 0..300 {
        let Some(&(_, to, _)) = sim.queue.front() else {
            break;
        };
        sim.deliver();
        while sim.resume() {}
        sim.tick(to);
    }

    assert!(sim.queue.len() > 300, "{} waiting", sim.queue.len());
}
