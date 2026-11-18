//! Blobs in a sealed group: read by whoever reads the group, and
//! held and served by anyone.

use super::super::tables::blobs::{create_with_pictures, insert_with, picture};
use super::*;
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
