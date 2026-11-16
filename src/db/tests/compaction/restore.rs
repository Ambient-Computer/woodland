//! Getting shed history back from a peer that kept it.

use super::*;
use crate::chain::Gossip;
use crate::db::entry::Entry;
use crate::db::message::{Message, Summary};
use crate::encoding;

fn restore(sim: &mut Sim, device: usize, group: GroupId) {
    let outputs = block_on(sim.devices[device].restore(group, group.into())).expect("asked");
    sim.absorb(device, outputs);
}

fn is_archive(sim: &Sim, device: usize, group: GroupId) -> bool {
    sim.devices[device].is_archive(group, group.into())
}

/// Every entry the device keeps, as stored.
fn log(sim: &Sim, device: usize, group: GroupId) -> Vec<(Vec<u8>, Vec<u8>)> {
    let (start, end) = keys::range(keys::logs(group));
    let view = sim.devices[device].store.view();

    block_on(crate::db::step::scan(&view, &start, &end)).expect("scanned")
}

/// Three devices write `count` notes between them, and the phone sheds
/// its history. The NAS keeps everything.
fn shed(count: usize) -> (Sim, GroupId) {
    let (mut sim, group) = Sim::group(3, 3);
    for note in 0..count {
        let device = [PHONE, LAPTOP, NAS][note % 3];
        let key = format!("note {note:04}");
        sim.write_to(device, group, set(notes(group), &key, "written"));
        if note % 50 == 49 {
            sim.settle();
        }
    }
    sim.settle();
    sim.compact(PHONE, group);

    (sim, group)
}

#[test]
fn history_comes_back_from_a_peer_that_kept_it() {
    let (mut sim, group) = shed(30);
    let read = all_notes(&sim, PHONE, group, true);
    assert_eq!(sim.entries_kept(PHONE, group), 0);
    assert!(!is_archive(&sim, PHONE, group));
    assert!(is_archive(&sim, NAS, group));

    restore(&mut sim, PHONE, group);
    sim.settle();

    assert_eq!(log(&sim, PHONE, group), log(&sim, NAS, group));
    assert!(is_archive(&sim, PHONE, group));
    assert_eq!(all_notes(&sim, PHONE, group, true), read);
}

#[test]
fn a_long_history_comes_back_a_part_at_a_time() {
    let (mut sim, group) = shed(300);
    sim.disconnect_all();
    sim.connect(PHONE, NAS);
    sim.run();

    restore(&mut sim, PHONE, group);
    sim.deliver();
    sim.deliver();
    let part = sim.entries_kept(PHONE, group);
    assert_eq!(part, 64, "one message's worth, all of it kept");

    // What's back stays back, and the rest follows, across a restart.
    sim.queue.clear();
    sim.restart(PHONE);
    assert_eq!(sim.entries_kept(PHONE, group), part);
    sim.connect(PHONE, NAS);
    sim.run();

    assert_eq!(log(&sim, PHONE, group), log(&sim, NAS, group));
    assert!(is_archive(&sim, PHONE, group));
}

/// Delivers everything, and returns how many messages carried history.
fn history_sent(sim: &mut Sim) -> usize {
    let mut messages = 0;
    while let Some((_, _, message)) = sim.queue.front() {
        let message: Message = encoding::decode(message).expect("a message");
        messages += usize::from(!message.restored.is_empty());
        sim.deliver();
    }

    messages
}

#[test]
fn nothing_comes_back_unasked() {
    let (mut sim, group) = shed(30);
    sim.disconnect_all();

    sim.connect(PHONE, NAS);
    let mut sent = history_sent(&mut sim);
    assert_eq!(sim.entries_kept(PHONE, group), 0);
    sim.write_to(NAS, group, set(notes(group), "more", "written"));
    sent += history_sent(&mut sim);

    assert_eq!(sent, 0);
}

/// The NAS doesn't wait to be asked for each part. It carries on from
/// what it last sent whenever it has something to say.
#[test]
fn history_is_sent_once() {
    let (mut sim, group) = shed(300);
    sim.disconnect_all();
    sim.connect(PHONE, NAS);
    sim.run();
    restore(&mut sim, PHONE, group);
    sim.deliver();

    // The NAS has sent some, and has other things to say meanwhile.
    for note in 0..3 {
        let key = format!("more {note}");
        sim.write_to(NAS, group, set(notes(group), &key, "written"));
    }
    let waiting = sim.queue.iter().filter(|(from, ..)| *from == NAS);
    let history = waiting.flat_map(|(.., message)| {
        let message: Message = encoding::decode(message).expect("a message");

        message.restored
    });
    let sent: Vec<_> = history.map(|entry| entry.link().hash).collect();
    let distinct: BTreeSet<_> = sent.iter().copied().collect();

    assert!(sent.len() > 64);
    assert_eq!(sent.len(), distinct.len());
}

#[test]
fn shedding_again_stops_it() {
    let (mut sim, group) = shed(300);
    sim.disconnect_all();
    sim.connect(PHONE, NAS);
    sim.run();
    restore(&mut sim, PHONE, group);
    sim.deliver();
    sim.deliver();
    assert!(sim.entries_kept(PHONE, group) > 0);

    sim.compact(PHONE, group);
    sim.settle();

    assert_eq!(sim.entries_kept(PHONE, group), 0);
    assert!(!is_archive(&sim, PHONE, group));
}

#[test]
fn a_device_that_caught_up_from_state_gets_the_history_too() {
    let (mut sim, group) = shed(30);
    sim.disconnect_all();
    sim.rejoin(LAPTOP, group, PHONE);
    sim.connect(LAPTOP, PHONE);
    sim.settle();
    assert_eq!(sim.entries_kept(LAPTOP, group), 0);
    assert_eq!(all_notes(&sim, LAPTOP, group, true).len(), 30);

    restore(&mut sim, LAPTOP, group);
    sim.connect(LAPTOP, NAS);
    sim.settle();

    assert_eq!(log(&sim, LAPTOP, group), log(&sim, NAS, group));
}

#[test]
fn restored_history_is_handed_on() {
    let (mut sim, group) = shed(30);
    sim.compact(LAPTOP, group);
    restore(&mut sim, PHONE, group);
    sim.settle();
    sim.disconnect_all();

    restore(&mut sim, LAPTOP, group);
    sim.connect(LAPTOP, PHONE);
    sim.settle();

    assert_eq!(log(&sim, LAPTOP, group), log(&sim, NAS, group));
}

/// Sends `to` these entries as if they were history it asked for.
fn offer(sim: &mut Sim, from: usize, to: usize, group: GroupId, restored: Vec<Entry>) {
    let message = Message {
        group,
        summary: Summary::default(),
        chain: Gossip::default(),
        entries: Vec::new(),
        restored,
        proofs: Vec::new(),
        snapshot: None,
    };
    let from = sim.id(from);
    let received = block_on(sim.devices[to].receive(from, &encoding::encode(&message)));
    sim.absorb(to, received.expect("received"));
}

#[test]
fn only_the_entry_that_comes_next_is_kept() {
    let (mut sim, group) = shed(30);
    sim.disconnect_all();
    restore(&mut sim, PHONE, group);
    let entry = |(_, stored): &(Vec<u8>, Vec<u8>)| encoding::decode::<Entry>(stored);
    let kept: Vec<Entry> = log(&sim, NAS, group).iter().filter_map(entry).collect();
    let phones = |entry: &&Entry| entry.header.author() == sim.devices[PHONE].identity.peer();
    let mut chain: Vec<Entry> = kept.iter().filter(phones).cloned().collect();
    chain.sort_by_key(|entry| entry.header.payload().seq);
    let last = chain.pop().expect("the phone wrote");

    // Not the latest of a chain: there's nothing to check it against.
    offer(&mut sim, NAS, PHONE, group, chain.clone());
    assert_eq!(sim.entries_kept(PHONE, group), 0);

    // The latest, with something else in it.
    let mut forged = last.clone();
    forged.payload = chain[0].payload.clone();
    offer(&mut sim, NAS, PHONE, group, vec![forged]);
    assert_eq!(sim.entries_kept(PHONE, group), 0);

    // Another entry the phone's key signed for the same place.
    let mut header = last.header.payload().clone();
    header.lamport += 1;
    let other = Entry::author(&sim.devices[PHONE].identity, header, Vec::new());
    offer(&mut sim, NAS, PHONE, group, vec![other.entry]);
    assert_eq!(sim.entries_kept(PHONE, group), 0);

    // Nothing is kept by a device that didn't ask.
    offer(&mut sim, NAS, LAPTOP, group, vec![last.clone()]);
    let before = sim.entries_kept(LAPTOP, group);
    sim.compact(LAPTOP, group);
    offer(&mut sim, NAS, LAPTOP, group, vec![last.clone()]);
    assert!(before > 0);
    assert_eq!(sim.entries_kept(LAPTOP, group), 0);

    // The latest and then the rest, latest first.
    chain.reverse();
    offer(&mut sim, NAS, PHONE, group, vec![last]);
    assert_eq!(sim.entries_kept(PHONE, group), 1);
    offer(&mut sim, NAS, PHONE, group, chain);
    assert_eq!(sim.entries_kept(PHONE, group), 10);
}
