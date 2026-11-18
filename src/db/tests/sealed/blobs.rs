//! Blobs in a sealed group: read by whoever reads the group, and
//! held and served by anyone.

use super::super::tables::blobs::{create_with_pictures, insert_with, picture};
use super::*;
use crate::db::message::Message;
use crate::encoding;
use crate::model::{Table, Value};
use crate::{BlobPolicy, BlobRef};

fn table(group: GroupId) -> ModelId {
    ModelId::table(group, [1; 32])
}

/// Bytes with the needle all through them.
fn content(length: usize) -> Vec<u8> {
    NEEDLE.bytes().cycle().take(length).collect()
}

fn policy(sim: &mut Sim, device: usize, policy: BlobPolicy) {
    let outputs = block_on(sim.devices[device].set_blob_policy(policy)).expect("said");
    sim.absorb(device, outputs);
}

fn held(sim: &Sim, device: usize, blob: &BlobRef) -> u64 {
    block_on(sim.devices[device].blob_held(blob)).expect("counted")
}

fn read(sim: &Sim, device: usize, blob: &BlobRef) -> Option<Vec<u8>> {
    let length = usize::try_from(blob.length()).expect("small");

    block_on(sim.devices[device].read_blob(blob, 0, length)).expect("read")
}

/// Whether the needle is anywhere in what the device stores, in a
/// group or out of one.
fn stores_anywhere(sim: &Sim, device: usize) -> bool {
    let view = sim.devices[device].store.view();
    let stored = block_on(crate::db::step::scan(&view, &[0], &[0xFF])).expect("scanned");
    let found = |bytes: &Vec<u8>| {
        bytes
            .windows(NEEDLE.len())
            .any(|it| it == NEEDLE.as_bytes())
    };

    stored.iter().any(|(key, value)| found(key) || found(value))
}

/// A sealed group with a picture the phone brought in.
fn pictured(length: usize) -> (Sim, GroupId, BlobRef) {
    let (mut sim, group) = sealed();
    for device in [LAPTOP, NAS, FRIEND, GUEST] {
        policy(&mut sim, device, BlobPolicy::All);
    }
    create_with_pictures(&mut sim, PHONE, table(group));
    let blob = sim.import_sealed(PHONE, &content(length), 1);
    insert_with(&mut sim, PHONE, table(group), "ada", blob);
    sim.settle();

    (sim, group, blob)
}

#[test]
fn a_sealed_blob_is_read_by_readers_and_carried_by_everyone_else() {
    let (sim, group, blob) = pictured(300_000);

    assert!(blob.is_sealed());
    assert_ne!(blob.name(), *blake3::hash(&content(300_000)).as_bytes());
    for device in [PHONE, LAPTOP, FRIEND] {
        assert_eq!(
            read(&sim, device, &blob),
            Some(content(300_000)),
            "device {device}"
        );
    }
    for device in [PHONE, LAPTOP, NAS, FRIEND, GUEST] {
        assert_eq!(held(&sim, device, &blob), 300_000, "device {device}");
        assert!(!stores_anywhere(&sim, device), "device {device}");
        assert!(!stores(&sim, device, group, NEEDLE), "device {device}");
    }
}

/// A range is read without the rest.
#[test]
fn a_range_of_a_sealed_blob_is_read_from_anywhere() {
    let (sim, _, blob) = pictured(300_000);
    let whole = content(300_000);

    for (offset, length) in [(0, 10), (114_000, 2000), (299_990, 100)] {
        let found = block_on(sim.devices[FRIEND].read_blob(&blob, offset, length)).expect("read");
        let start = usize::try_from(offset).expect("small");
        let end = (start + length).min(whole.len());

        assert_eq!(found.as_deref(), Some(&whole[start..end]), "{offset}");
    }
}

/// A device with no keys serves what it holds, so a blob gets to a
/// reader that never meets its author.
#[test]
fn a_device_that_cant_read_passes_a_blob_on() {
    let (mut sim, group) = sealed();
    policy(&mut sim, GUEST, BlobPolicy::All);
    policy(&mut sim, FRIEND, BlobPolicy::All);
    create_with_pictures(&mut sim, PHONE, table(group));
    sim.settle();
    sim.disconnect_all();
    sim.connect(PHONE, GUEST);
    let blob = sim.import_sealed(PHONE, &content(200_000), 1);
    insert_with(&mut sim, PHONE, table(group), "ada", blob);
    sim.settle();
    assert_eq!(held(&sim, GUEST, &blob), 200_000);

    sim.disconnect_all();
    sim.connect(GUEST, FRIEND);
    sim.settle();

    assert_eq!(read(&sim, FRIEND, &blob), Some(content(200_000)));
}

/// A key encrypts one blob.
#[test]
fn the_same_bytes_brought_in_twice_are_two_blobs() {
    let (mut sim, _) = sealed();
    let first = sim.import_sealed(PHONE, &content(50_000), 1);
    let second = sim.import_sealed(PHONE, &content(50_000), 2);

    assert_ne!(first.name(), second.name());
    assert_eq!(read(&sim, PHONE, &first), read(&sim, PHONE, &second));
}

#[test]
fn a_reference_is_for_one_kind_of_group() {
    let (mut sim, group) = sealed();
    create_with_pictures(&mut sim, PHONE, table(group));
    let open = sim.import(PHONE, &content(1000));
    let cells = [(picture(table(group)), Value::from(open))];
    let insert = Transaction::new().insert(table(group), "ada", cells);
    let written = block_on(sim.devices[PHONE].write(group, insert));
    assert!(matches!(written, Err(Error::BlobMisplaced)));

    let mut cluster = household();
    let people = ModelId::table(cluster.home, [1; 32]);
    create_with_pictures(&mut cluster, PHONE, people);
    let sealed = cluster.import_sealed(PHONE, &content(1000), 1);
    let cells = [(picture(people), Value::from(sealed))];
    let insert = Transaction::new().insert(people, "ada", cells);
    let written = block_on(cluster.devices[PHONE].write(cluster.home, insert));
    assert!(matches!(written, Err(Error::BlobMisplaced)));
}

/// Replacing the group's secret re-encrypts no blob and renames none.
#[test]
fn a_blob_outlives_the_secret_it_was_written_under() {
    let (mut sim, group, blob) = pictured(150_000);
    let roles: Vec<_> = roles(&sim)
        .into_iter()
        .filter(|(device, _)| *device != FRIEND)
        .collect();
    sim.assign(PHONE, group, roles);
    sim.settle();
    assert!(!sim.devices[FRIEND].reads(group));

    // The laptop lets it go, and fetches it again under the new secret.
    let outputs = block_on(sim.devices[LAPTOP].release_blob(&blob)).expect("let go");
    sim.absorb(LAPTOP, outputs);
    sim.settle();
    assert_eq!(held(&sim, LAPTOP, &blob), 0);
    let outputs = block_on(sim.devices[LAPTOP].fetch_blob(&blob)).expect("asked");
    sim.absorb(LAPTOP, outputs);
    sim.settle();

    assert_eq!(read(&sim, LAPTOP, &blob), Some(content(150_000)));

    // And a copy of the reference, written since, is to the same blob.
    let cells = [(picture(table(group)), Value::from(blob))];
    sim.write_to(
        LAPTOP,
        group,
        Transaction::new().insert(table(group), "bob", cells),
    );
    sim.settle();
    let found = Table::read(&sim.reader(PHONE).read(&Query::table(group, table(group))));
    let pictures = found
        .iter()
        .flat_map(Table::rows)
        .map(|row| row.get(picture(table(group))).clone());

    assert_eq!(
        pictures.collect::<Vec<_>>(),
        [Value::Blob(blob), Value::Blob(blob)]
    );
}

/// The readers write something more and shed all their history, so
/// that whoever is behind is handed state.
fn shed(sim: &mut Sim, group: GroupId, note: &str) {
    sim.write_to(PHONE, group, set(notes(group), "plan", note));
    sim.settle();
    for device in [PHONE, LAPTOP] {
        sim.compact(device, group);
    }
}

/// State a device can't read says which blobs it refers to, and
/// that's what the device goes by.
#[test]
fn a_device_handed_state_it_cant_read_fetches_the_blobs_it_lists() {
    let (mut sim, group, blob) = pictured(200_000);
    shed(&mut sim, group, "sunday");
    sim.rejoin(NAS, group, PHONE);
    policy(&mut sim, NAS, BlobPolicy::All);
    sim.connect(NAS, PHONE);
    sim.settle();

    assert_eq!(sim.entries_kept(NAS, group), 0, "it was handed state");
    assert_eq!(held(&sim, NAS, &blob), 200_000);
}

/// The entries that listed a blob go when newer state arrives. The
/// state lists it too, so the bytes stay: and go when it doesn't.
#[test]
fn a_device_that_cant_read_keeps_a_blob_for_as_long_as_the_state_lists_it() {
    let (mut sim, group, blob) = pictured(200_000);
    sim.isolate(GUEST);
    shed(&mut sim, group, "sunday");
    sim.connect(GUEST, PHONE);
    sim.settle();
    assert_eq!(sim.entries_kept(GUEST, group), 0, "it was handed state");
    assert_eq!(held(&sim, GUEST, &blob), 200_000);

    sim.isolate(GUEST);
    let found = Table::read(&sim.reader(PHONE).read(&Query::table(group, table(group))));
    let row = found
        .iter()
        .flat_map(Table::rows)
        .next()
        .expect("a row")
        .clone();
    sim.write_to(
        PHONE,
        group,
        Transaction::new().delete_row(table(group), &row),
    );
    sim.settle();
    shed(&mut sim, group, "monday");
    sim.connect(GUEST, PHONE);
    sim.settle();

    assert_eq!(held(&sim, GUEST, &blob), 0);
}

/// A device that can't read is never told what to drop by whoever
/// hands it state: the list is checked against the checkpoint.
#[test]
fn state_that_lists_other_blobs_than_the_checkpoints_is_refused() {
    let (mut sim, group, blob) = pictured(200_000);
    sim.isolate(GUEST);
    shed(&mut sim, group, "sunday");
    let behind = sim.devices[GUEST].groups[&group].folded();
    sim.connect(GUEST, PHONE);

    // Whatever state is on its way to the guest says it refers to
    // nothing.
    let guest = GUEST;
    loop {
        for (_, to, bytes) in &mut sim.queue {
            let Some(mut message) = encoding::decode::<Message>(bytes) else {
                continue;
            };
            let sealed = message
                .snapshot
                .as_mut()
                .and_then(|piece| piece.sealed.as_mut());
            if let (true, Some(sealed)) = (*to == guest, sealed) {
                sealed.blobs.clear();
                *bytes = encoding::encode(&message);
            }
        }
        if !(sim.deliver() || sim.resume() || sim.carry()) {
            break;
        }
    }

    assert_eq!(sim.devices[GUEST].groups[&group].folded(), behind);
    assert_eq!(held(&sim, GUEST, &blob), 200_000, "nothing was let go");
}

fn release(sim: &mut Sim, device: usize, blob: &BlobRef) {
    let outputs = block_on(sim.devices[device].release_blob(blob)).expect("released");
    sim.absorb(device, outputs);
}

/// A device that kept a blob it couldn't read keeps it when it's
/// given a key, though it starts the group over.
#[test]
fn a_device_given_a_key_keeps_the_blobs_it_held() {
    for length in [50_000, 2_000_000] {
        let (mut sim, group, blob) = pictured(length);
        assert_eq!(held(&sim, GUEST, &blob), length as u64);
        for device in [LAPTOP, NAS, FRIEND] {
            policy(&mut sim, device, BlobPolicy::None);
            release(&mut sim, device, &blob);
        }
        policy(&mut sim, PHONE, BlobPolicy::None);
        release(&mut sim, PHONE, &blob);
        sim.settle();

        let mut roles = roles(&sim).to_vec();
        roles.push((GUEST, Role::reader(key(&sim, GUEST))));
        sim.assign(PHONE, group, roles);
        sim.settle();

        assert!(sim.devices[GUEST].reads(group));
        assert_eq!(held(&sim, GUEST, &blob), length as u64, "length {length}");
    }
}

/// A sealed group with a picture, from which the friend was then
/// removed, and another picture brought in since.
fn shut_out() -> (Sim, GroupId, BlobRef, BlobRef) {
    let (mut sim, group, before) = pictured(200_000);
    let fewer = roles(&sim)
        .into_iter()
        .filter(|(device, _)| *device != FRIEND);
    sim.assign(PHONE, group, fewer);
    sim.settle();
    let after = sim.import_sealed(PHONE, &content(300_000), 2);
    insert_with(&mut sim, PHONE, table(group), "bob", after);
    sim.write_to(PHONE, group, set(notes(group), "plan", "after"));
    sim.settle();

    (sim, group, before, after)
}

fn relinquish(sim: &mut Sim, device: usize, group: GroupId) {
    let outputs = block_on(sim.devices[device].relinquish(group)).expect("given up");
    sim.absorb(device, outputs);
}

/// It can't read what's written now and has no state to stand in
/// for it, so it sheds none of it: nor the blobs it lists.
#[test]
fn a_reader_that_was_removed_sheds_nothing_until_it_chooses() {
    let (mut sim, group, before, after) = shut_out();
    assert!(sim.devices[FRIEND].is_shut_out(group));
    assert!(!sim.devices[PHONE].is_shut_out(group));
    assert!(!sim.devices[GUEST].is_shut_out(group), "it never read");
    let kept = sim.entries_kept(FRIEND, group);

    let shed = block_on(sim.devices[FRIEND].compact(group));
    let clock = table(group).clock();
    let one = block_on(sim.devices[FRIEND].compact_clock(group, clock));

    assert!(matches!(shed, Err(Error::ShutOut)), "{shed:?}");
    assert!(matches!(one, Err(Error::ShutOut)), "{one:?}");
    assert_eq!(sim.entries_kept(FRIEND, group), kept);
    assert_eq!(held(&sim, FRIEND, &before), 200_000);
    assert_eq!(held(&sim, FRIEND, &after), 300_000);
}

/// Let back in, it starts over and reads what was written while it
/// was out.
#[test]
fn a_reader_removed_and_let_back_in_catches_up() {
    let (mut sim, group, before, after) = shut_out();
    assert_eq!(read_note(&sim, FRIEND, group, "plan"), None);

    let all = roles(&sim);
    sim.assign(PHONE, group, all);
    sim.settle();

    assert!(!sim.devices[FRIEND].is_shut_out(group));
    assert_eq!(
        read_note(&sim, FRIEND, group, "plan").as_deref(),
        Some("after")
    );
    assert_eq!(held(&sim, FRIEND, &before), 200_000);
    assert_eq!(held(&sim, FRIEND, &after), 300_000);
    sim.compact(FRIEND, group);
}

/// What it gives up is everything it could read, and the means to
/// read it. From there it's a device that never could: it carries
/// the group, sheds when it's handed state, and keeps the blobs the
/// state lists.
#[test]
fn a_reader_that_was_removed_gives_up_what_it_read_to_carry_the_group() {
    let (mut sim, group, before, after) = shut_out();
    assert!(stores(&sim, FRIEND, group, NEEDLE) || read(&sim, FRIEND, &before).is_some());

    relinquish(&mut sim, FRIEND, group);
    sim.settle();
    assert!(sim.entries_kept(FRIEND, group) > 0);
    sim.compact(FRIEND, group);
    sim.settle();

    assert!(!sim.devices[FRIEND].is_shut_out(group));
    assert!(!sim.devices[FRIEND].reads(group));
    assert!(!stores(&sim, FRIEND, group, "after"));
    assert_eq!(read_note(&sim, FRIEND, group, "plan"), None);
    assert_eq!(sim.entries_kept(FRIEND, group), 0, "it was handed state");
    assert_eq!(
        sim.devices[FRIEND].groups[&group].folded(),
        sim.devices[PHONE].groups[&group].folded()
    );
    assert_eq!(held(&sim, FRIEND, &before), 200_000);
    assert_eq!(held(&sim, FRIEND, &after), 300_000);
    assert!(block_on(sim.devices[FRIEND].compact(group)).is_ok());
}

/// The chain still carries the copies of the secrets made for it.
/// It takes none of them: not after a restart either.
#[test]
fn secrets_given_up_stay_given_up_until_the_group_names_the_device_again() {
    let (mut sim, group, _, after) = shut_out();
    relinquish(&mut sim, FRIEND, group);
    sim.settle();
    sim.restart(FRIEND);
    sim.connect_all();
    sim.write_to(PHONE, group, set(notes(group), "more", "later"));
    sim.settle();
    assert!(!sim.devices[FRIEND].reads(group));
    assert!(!sim.devices[FRIEND].is_shut_out(group));
    assert_eq!(read_note(&sim, FRIEND, group, "plan"), None);

    let all = roles(&sim);
    sim.assign(PHONE, group, all);
    sim.settle();

    assert!(sim.devices[FRIEND].reads(group));
    assert_eq!(
        read_note(&sim, FRIEND, group, "plan").as_deref(),
        Some("after")
    );
    assert_eq!(
        read_note(&sim, FRIEND, group, "more").as_deref(),
        Some("later")
    );
    assert_eq!(held(&sim, FRIEND, &after), 300_000);
}

/// A device that reads has nothing to give up, and neither has one
/// that never did.
#[test]
fn only_a_device_that_can_no_longer_read_gives_anything_up() {
    let (mut sim, group, _, _) = shut_out();

    let reader = block_on(sim.devices[LAPTOP].relinquish(group));
    let relay = block_on(sim.devices[GUEST].relinquish(group));

    assert!(matches!(reader, Err(Error::StillReads)), "{reader:?}");
    assert!(matches!(relay, Err(Error::StillReads)), "{relay:?}");
}

/// Starting over lets go of everything that referred to a blob, to
/// be sent again. The bytes wait for that, however long it takes:
/// this device may be the only one that has them.
#[test]
fn a_device_that_starts_over_keeps_its_blobs_until_it_has_caught_up() {
    let (mut sim, group, before, after) = shut_out();
    for device in [PHONE, LAPTOP, NAS, GUEST] {
        policy(&mut sim, device, BlobPolicy::None);
        release(&mut sim, device, &before);
        release(&mut sim, device, &after);
    }
    sim.settle();

    sim.isolate(FRIEND);
    relinquish(&mut sim, FRIEND, group);
    while sim.resume() {}
    sim.restart(FRIEND);
    while sim.resume() {}
    assert_eq!(held(&sim, FRIEND, &before), 200_000);
    assert_eq!(held(&sim, FRIEND, &after), 300_000);

    assert!(holds_over(&sim, FRIEND, group));

    sim.connect_all();
    sim.settle();
    assert!(!holds_over(&sim, FRIEND, group));
    assert_eq!(held(&sim, FRIEND, &before), 200_000);
    assert_eq!(held(&sim, FRIEND, &after), 300_000);
}

/// Whether the device still keeps blobs for what the group referred
/// to before it started over.
fn holds_over(sim: &Sim, device: usize, group: GroupId) -> bool {
    let view = sim.devices[device].store.view();
    let kept = block_on(crate::store::View::get(&view, &keys::held_over(group))).expect("read");

    kept.is_some()
}

/// Once the group has been sent again, what refers to a blob is what
/// came back. A blob the group stopped referring to meanwhile goes.
#[test]
fn a_blob_nothing_refers_to_once_the_group_is_back_is_let_go() {
    let (mut sim, group, before, after) = shut_out();
    sim.isolate(FRIEND);
    relinquish(&mut sim, FRIEND, group);
    while sim.resume() {}

    let found = Table::read(&sim.reader(PHONE).read(&Query::table(group, table(group))));
    let rows: Vec<_> = found.iter().flat_map(Table::rows).cloned().collect();
    for row in &rows {
        sim.write_to(
            PHONE,
            group,
            Transaction::new().delete_row(table(group), row),
        );
    }
    sim.settle();
    shed(&mut sim, group, "sunday");
    assert_eq!(held(&sim, FRIEND, &before), 200_000);
    assert_eq!(held(&sim, FRIEND, &after), 300_000);

    sim.connect_all();
    sim.settle();

    assert!(!holds_over(&sim, FRIEND, group));
    assert_eq!(held(&sim, FRIEND, &before), 0);
    assert_eq!(held(&sim, FRIEND, &after), 0);
}

/// A device that can't read has every entry, and nobody hands it
/// state unasked. Compacting is how it asks: it sheds what the state
/// stands in for, and keeps the blobs the state lists.
#[test]
fn a_device_that_cant_read_compacts_by_asking_for_state() {
    let (mut sim, group, blob) = pictured(200_000);
    assert!(sim.entries_kept(GUEST, group) > 0);
    let height = sim.devices[PHONE].groups[&group].folded();

    sim.compact(GUEST, group);
    sim.settle();

    assert_eq!(sim.entries_kept(GUEST, group), 0);
    assert_eq!(sim.devices[GUEST].groups[&group].folded(), height);
    assert_eq!(held(&sim, GUEST, &blob), 200_000);
    assert!(!stores_anywhere(&sim, GUEST));

    // It carries on as before, and passes on what it was handed.
    sim.write_to(PHONE, group, set(notes(group), "plan", "sunday"));
    sim.settle();
    assert_eq!(sim.entries_kept(GUEST, group), 1);
    sim.rejoin(FRIEND, group, GUEST);
    policy(&mut sim, FRIEND, BlobPolicy::All);
    sim.isolate(FRIEND);
    sim.connect(FRIEND, GUEST);
    sim.settle();
    assert_eq!(held(&sim, FRIEND, &blob), 200_000);
}
