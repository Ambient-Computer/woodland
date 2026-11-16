//! Getting shed history back from a peer that kept it.

use super::*;
use crate::chain::{Certificate, Gossip};
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
        links: Vec::new(),
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

fn height(sim: &Sim, device: usize, group: GroupId) -> usize {
    sim.devices[device].groups[&group].checkpoints()
}

/// The laptop is away while the others write, over many checkpoints.
/// The phone sheds everything and the NAS keeps it.
fn left_behind() -> (Sim, GroupId) {
    let (mut sim, group) = Sim::group(3, 3);
    sim.settle();
    sim.isolate(LAPTOP);
    for note in 0..40 {
        let device = [PHONE, NAS][note % 2];
        let key = format!("note {note:04}");
        sim.write_to(device, group, set(notes(group), &key, "written"));
        sim.settle();
    }
    sim.compact(PHONE, group);

    (sim, group)
}

#[test]
fn checkpoints_come_back_with_the_history() {
    let (mut sim, group) = left_behind();
    let all = height(&sim, NAS, group);
    assert!(all > 20, "the test means to have many checkpoints");
    assert!(height(&sim, PHONE, group) < all);

    restore(&mut sim, PHONE, group);
    assert!(!is_archive(&sim, PHONE, group));
    sim.settle();

    assert_eq!(height(&sim, PHONE, group), all);
    assert!(is_archive(&sim, PHONE, group));
    sim.restart(PHONE);
    assert_eq!(height(&sim, PHONE, group), all);
    assert!(is_archive(&sim, PHONE, group));
}

/// A device that's far behind gets state from one that shed its
/// history, and history from one that has it back.
#[test]
fn a_device_with_its_history_back_brings_another_up_to_date_with_it() {
    let (mut sim, group) = left_behind();
    restore(&mut sim, PHONE, group);
    sim.settle();
    sim.disconnect_all();

    sim.connect(LAPTOP, PHONE);
    sim.settle();

    assert_eq!(log(&sim, LAPTOP, group), log(&sim, NAS, group));
    assert_eq!(height(&sim, LAPTOP, group), height(&sim, NAS, group));
    assert_eq!(all_notes(&sim, LAPTOP, group, true).len(), 40);
}

#[test]
fn a_device_that_shed_its_history_brings_another_up_to_date_with_state() {
    let (mut sim, group) = left_behind();
    sim.disconnect_all();

    sim.connect(LAPTOP, PHONE);
    sim.settle();

    assert_eq!(sim.entries_kept(LAPTOP, group), 0);
    assert_eq!(all_notes(&sim, LAPTOP, group, true).len(), 40);
}

#[test]
fn a_checkpoint_comes_back_only_into_the_gap_it_fills() {
    let (mut sim, group) = left_behind();
    sim.disconnect_all();
    let held = height(&sim, PHONE, group);
    let links = sim.devices[NAS].groups[&group].links_below(u64::MAX);
    let offer = |sim: &mut Sim, links: Vec<Certificate>| {
        let message = Message {
            group,
            summary: Summary::default(),
            chain: Gossip::default(),
            entries: Vec::new(),
            links,
            restored: Vec::new(),
            proofs: Vec::new(),
            snapshot: None,
        };
        let from = sim.id(NAS);
        let received = block_on(sim.devices[PHONE].receive(from, &encoding::encode(&message)));
        sim.absorb(PHONE, received.expect("received"));
    };

    // It didn't ask.
    offer(&mut sim, links.clone());
    assert_eq!(height(&sim, PHONE, group), held);

    // Oldest first: nothing held names any of them yet.
    restore(&mut sim, PHONE, group);
    let mut oldest_first = links.clone();
    oldest_first.reverse();
    oldest_first.truncate(links.len() - 2);
    offer(&mut sim, oldest_first);
    assert_eq!(height(&sim, PHONE, group), held);

    // The right checkpoints, without the votes that decided them.
    let mut bare = links.clone();
    for link in &mut bare {
        link.votes.clear();
    }
    offer(&mut sim, bare);
    assert_eq!(height(&sim, PHONE, group), held);

    offer(&mut sim, links);
    assert_eq!(height(&sim, PHONE, group), height(&sim, NAS, group));
}

/// The heights of the checkpoints in every message waiting to go.
fn links_waiting(sim: &Sim) -> Vec<u64> {
    let links = sim.queue.iter().flat_map(|(.., message)| {
        let message: Message = encoding::decode(message).expect("a message");

        message.links
    });

    links.map(|link| link.checkpoint.height).collect()
}

#[test]
fn checkpoints_come_back_a_part_at_a_time_and_once() {
    let (mut sim, group) = left_behind();
    sim.disconnect_all();
    sim.connect(PHONE, NAS);
    sim.run();
    assert_eq!(links_waiting(&sim), []);
    sim.write_to(NAS, group, set(notes(group), "unasked", "written"));
    assert_eq!(links_waiting(&sim), []);
    sim.run();

    restore(&mut sim, PHONE, group);
    sim.deliver();
    assert_eq!(links_waiting(&sim).len(), 16);

    // The NAS carries on from there when it next has something to say.
    for note in 0..2 {
        let key = format!("more {note}");
        sim.write_to(NAS, group, set(notes(group), &key, "written"));
    }
    let sent = links_waiting(&sim);
    let distinct: BTreeSet<u64> = sent.iter().copied().collect();
    assert!(sent.len() > 16);
    assert_eq!(sent.len(), distinct.len());
}

/// The phone has asked for its checkpoints back, and has some of them.
fn partly_mended() -> (Sim, GroupId, usize) {
    let (mut sim, group) = left_behind();
    sim.disconnect_all();
    sim.connect(PHONE, NAS);
    sim.run();
    let before = height(&sim, PHONE, group);
    restore(&mut sim, PHONE, group);
    sim.deliver();
    sim.deliver();
    let part = height(&sim, PHONE, group);
    assert!(before < part && part < height(&sim, NAS, group));
    sim.queue.clear();

    (sim, group, part)
}

#[test]
fn checkpoints_keep_coming_back_after_a_restart() {
    let (mut sim, group, part) = partly_mended();

    sim.restart(PHONE);
    assert_eq!(height(&sim, PHONE, group), part);
    sim.connect(PHONE, NAS);
    sim.run();

    assert_eq!(height(&sim, PHONE, group), height(&sim, NAS, group));
}

#[test]
fn shedding_again_stops_checkpoints_coming_back() {
    let (mut sim, group, part) = partly_mended();

    sim.compact(PHONE, group);
    let shed = height(&sim, PHONE, group);
    sim.settle();

    assert!(shed < part);
    assert_eq!(height(&sim, PHONE, group), shed);
}
