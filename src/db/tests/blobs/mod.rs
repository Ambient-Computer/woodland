//! The bytes of blobs on one device: brought in, read back, counted,
//! and let go.

mod fetching;

use super::tables::blobs::{bytes, create_with_pictures, insert_with, picture};
use super::tables::{people, read_table};
use super::*;
use crate::BlobRef;
use crate::model::Value;

/// Everything the device's store keeps for blobs, but for what the
/// device has said about itself.
fn kept(sim: &Sim, device: usize) -> usize {
    let (start, end) = keys::range(vec![5]);
    let view = sim.devices[device].store.view();
    let stored = block_on(crate::db::step::scan(&view, &start, &end)).expect("scanned");
    let apart = [crate::blob::keys::next(), crate::blob::keys::policy()];

    stored
        .iter()
        .filter(|(key, _)| !apart.contains(key))
        .count()
}

fn read(sim: &Sim, device: usize, blob: &BlobRef, offset: u64, length: usize) -> Option<Vec<u8>> {
    block_on(sim.devices[device].read_blob(blob, offset, length)).expect("read")
}

/// All of a blob, read a step's worth at a time.
fn all(sim: &Sim, device: usize, blob: &BlobRef) -> Option<Vec<u8>> {
    let mut found = Vec::new();
    while (found.len() as u64) < blob.length() {
        let next = read(sim, device, blob, found.len() as u64, usize::MAX)?;
        assert_ne!(next.len(), 0);
        found.extend(next);
    }

    Some(found)
}

fn imports(sim: &Sim, device: usize) -> Vec<crate::Import> {
    block_on(sim.devices[device].imports()).expect("listed")
}

#[test]
fn a_blob_is_named_by_the_hash_of_its_bytes() {
    let (mut sim, _) = Sim::group(1, 1);
    for length in [0, 1, 16_384, 16_385, 114_688, 300_000, 2_500_000] {
        let content = bytes(1, length);
        let blob = sim.import(PHONE, &content);

        assert_eq!(blob.name(), *blake3::hash(&content).as_bytes(), "{length}");
        assert_eq!(blob.length(), length as u64);
        assert_eq!(all(&sim, PHONE, &blob), Some(content), "{length}");
    }
}

/// Past a page of hashes, the name is made of the pages'.
#[test]
fn a_blob_longer_than_a_page_of_hashes_has_the_same_name() {
    let (mut sim, _) = Sim::group(1, 1);
    let content = bytes(2, 2049 * 16_384 + 5);
    let blob = sim.import(PHONE, &content);

    assert_eq!(blob.name(), *blake3::hash(&content).as_bytes());
    let tail = read(&sim, PHONE, &blob, blob.length() - 9, 100);
    assert_eq!(tail.as_deref(), Some(&content[content.len() - 9..]));
}

#[test]
fn a_range_is_read_from_anywhere_in_a_blob() {
    let (mut sim, _) = Sim::group(1, 1);
    let content = bytes(3, 400_000);
    let blob = sim.import(PHONE, &content);

    let middle = read(&sim, PHONE, &blob, 114_000, 2000);
    assert_eq!(middle.as_deref(), Some(&content[114_000..116_000]));
    let end = read(&sim, PHONE, &blob, 399_990, 2000);
    assert_eq!(end.as_deref(), Some(&content[399_990..]));
    assert_eq!(read(&sim, PHONE, &blob, 400_000, 10), Some(Vec::new()));
}

#[test]
fn a_blob_that_isnt_held_says_so() {
    let mut cluster = household();
    let blob = cluster.import(PHONE, &bytes(4, 5000));

    assert_eq!(read(&cluster, LAPTOP, &blob, 0, 10), None);
    assert_eq!(
        block_on(cluster.devices[LAPTOP].blob_held(&blob)).ok(),
        Some(0)
    );
    assert_eq!(
        block_on(cluster.devices[PHONE].blob_held(&blob)).ok(),
        Some(5000)
    );
}

#[test]
fn an_import_is_listed_until_something_refers_to_it() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create_with_pictures(&mut cluster, PHONE, table);
    let (id, _) = block_on(cluster.devices[PHONE].begin_import()).expect("begun");
    let (_, outputs) =
        block_on(cluster.devices[PHONE].import(id, &bytes(5, 3000))).expect("imported");
    drop(outputs);

    let unfinished = imports(&cluster, PHONE);
    assert_eq!(unfinished.len(), 1);
    assert_eq!((unfinished[0].length, unfinished[0].blob), (3000, None));

    let (blob, _) = block_on(cluster.devices[PHONE].finish_import(id)).expect("finished");
    assert_eq!(imports(&cluster, PHONE)[0].blob, Some(blob));

    insert_with(&mut cluster, PHONE, table, "ada", blob);
    assert_eq!(imports(&cluster, PHONE), []);
    assert_eq!(all(&cluster, PHONE, &blob), Some(bytes(5, 3000)));
}

#[test]
fn an_abandoned_import_leaves_nothing_behind() {
    let (mut sim, _) = Sim::group(1, 1);
    let (id, _) = block_on(sim.devices[PHONE].begin_import()).expect("begun");
    let (_, outputs) =
        block_on(sim.devices[PHONE].import(id, &bytes(6, 900_000))).expect("imported");
    drop(outputs);
    let finished = sim.import(PHONE, &bytes(7, 5_000_000));
    let (other, _) = imports(&sim, PHONE)
        .into_iter()
        .find_map(|import| import.blob.map(|blob| (import.id, blob)))
        .expect("a finished import");
    assert_ne!(other, id);

    for import in [id, other] {
        let outputs = block_on(sim.devices[PHONE].abandon_import(import)).expect("abandoned");
        sim.absorb(PHONE, outputs);
    }
    assert!(kept(&sim, PHONE) > 0, "deleted across steps");
    sim.settle();

    assert_eq!(kept(&sim, PHONE), 0);
    assert_eq!(read(&sim, PHONE, &finished, 0, 10), None);
    let again = block_on(sim.devices[PHONE].abandon_import(id));
    assert!(matches!(again, Err(Error::UnknownImport)));
}

#[test]
fn the_same_bytes_are_kept_once() {
    let (mut sim, _) = Sim::group(1, 1);
    let content = bytes(8, 300_000);
    let first = sim.import(PHONE, &content);
    let before = kept(&sim, PHONE);
    let second = sim.import(PHONE, &content);
    sim.settle();

    assert_eq!(first, second);
    assert_eq!(kept(&sim, PHONE), before);
    assert_eq!(imports(&sim, PHONE).len(), 1);
    assert_eq!(all(&sim, PHONE, &first), Some(content));
}

/// A device refers only to what it could hand over.
#[test]
fn a_reference_is_written_only_by_a_device_that_holds_the_blob() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create_with_pictures(&mut cluster, PHONE, table);
    let blob = cluster.import(PHONE, &bytes(9, 1000));
    insert_with(&mut cluster, PHONE, table, "ada", blob);
    cluster.settle();

    let cells = [(picture(table), Value::from(blob))];
    let copy = Transaction::new().insert(table, "bob", cells);
    let written = block_on(cluster.devices[LAPTOP].write(cluster.home, copy));

    assert!(matches!(written, Err(Error::BlobNotHeld)));
}

#[test]
fn bytes_go_when_nothing_refers_to_them() {
    let mut cluster = household();
    let table = people(&cluster, 1);
    create_with_pictures(&mut cluster, PHONE, table);
    let blob = cluster.import(PHONE, &bytes(10, 600_000));
    insert_with(&mut cluster, PHONE, table, "ada", blob);
    cluster.settle();

    let read = read_table(&cluster, PHONE, table).expect("a table");
    let row = read.rows().first().expect("a row");
    cluster.write(PHONE, Transaction::new().delete_row(table, row));
    cluster.settle();

    // The entry that wrote it is still held, and may yet count.
    assert_eq!(all(&cluster, PHONE, &blob), Some(bytes(10, 600_000)));

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        cluster.compact(device, cluster.home);
    }
    cluster.settle();

    assert_eq!(read_blob_anywhere(&cluster, &blob), None);
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(kept(&cluster, device), 0, "device {device}");
    }
}

fn read_blob_anywhere(cluster: &Sim, blob: &BlobRef) -> Option<Vec<u8>> {
    let mut devices = 0..cluster.devices.len();

    devices.find_map(|device| read(cluster, device, blob, 0, 10))
}

/// One copy serves every value that refers to it, and goes with the
/// last.
#[test]
fn a_blob_two_rows_refer_to_outlives_one_of_them() {
    let (mut sim, group) = Sim::group(1, 1);
    let table = ModelId::table(group, [1; 32]);
    create_with_pictures(&mut sim, PHONE, table);
    let blob = sim.import(PHONE, &bytes(11, 200_000));
    insert_with(&mut sim, PHONE, table, "ada", blob);
    insert_with(&mut sim, PHONE, table, "bob", blob);
    sim.settle();

    let remove = |sim: &mut Sim, at: usize| {
        let read = read_table(sim, PHONE, table).expect("a table");
        let row = read.rows().get(at).expect("a row");
        sim.write(PHONE, Transaction::new().delete_row(table, row));
        sim.settle();
        sim.compact(PHONE, group);
        sim.settle();
    };
    remove(&mut sim, 0);
    assert_eq!(all(&sim, PHONE, &blob), Some(bytes(11, 200_000)));

    remove(&mut sim, 0);
    assert_eq!(read(&sim, PHONE, &blob, 0, 10), None);
    assert_eq!(kept(&sim, PHONE), 0);
}

/// A reference that arrives while the bytes are going keeps what's
/// left of them.
#[test]
fn a_blob_referred_to_again_while_its_bytes_go_is_held_in_part() {
    let (mut sim, group) = Sim::group(1, 1);
    let table = ModelId::table(group, [1; 32]);
    create_with_pictures(&mut sim, PHONE, table);
    let content = bytes(12, 8_000_000);
    let blob = sim.import(PHONE, &content);
    let (id, _) = imports(&sim, PHONE)
        .into_iter()
        .map(|import| (import.id, import.blob))
        .next()
        .expect("an import");
    let outputs = block_on(sim.devices[PHONE].abandon_import(id)).expect("abandoned");
    sim.absorb(PHONE, outputs);
    let outputs = block_on(sim.devices[PHONE].resume()).expect("resumed");
    sim.absorb(PHONE, outputs);

    let held = block_on(sim.devices[PHONE].blob_held(&blob)).expect("counted");
    assert!(0 < held && held < blob.length(), "{held}");

    // Nothing but the whole blob may be written, and the import that
    // brings back the rest takes the place of what was left.
    let cells = [(picture(table), Value::from(blob))];
    let insert = Transaction::new().insert(table, "ada", cells);
    let written = block_on(sim.devices[PHONE].write(group, insert));
    assert!(matches!(written, Err(Error::BlobNotHeld)));

    let again = sim.import(PHONE, &content);
    insert_with(&mut sim, PHONE, table, "ada", again);
    sim.settle();

    assert_eq!(again, blob);
    assert_eq!(all(&sim, PHONE, &blob), Some(content));
}

/// A device that stops partway carries on where it was, and the blob
/// is the one it would have been.
#[test]
fn an_import_carries_on_after_a_restart() {
    let (mut sim, _) = Sim::group(1, 1);
    let content = bytes(13, 700_000);
    let (id, _) = block_on(sim.devices[PHONE].begin_import()).expect("begun");
    let (taken, outputs) =
        block_on(sim.devices[PHONE].import(id, &content[..250_000])).expect("imported");
    drop(outputs);
    sim.restart(PHONE);

    let unfinished = imports(&sim, PHONE);
    assert_eq!(unfinished.len(), 1);
    assert_eq!((unfinished[0].id, unfinished[0].length), (id, taken as u64));

    let mut rest = &content[taken..];
    while !rest.is_empty() {
        let (taken, outputs) = block_on(sim.devices[PHONE].import(id, rest)).expect("imported");
        drop(outputs);
        rest = &rest[taken..];
    }
    let (blob, _) = block_on(sim.devices[PHONE].finish_import(id)).expect("finished");

    assert_eq!(blob.name(), *blake3::hash(&content).as_bytes());
    assert_eq!(all(&sim, PHONE, &blob), Some(content));
}

/// Deleting carries on after a restart too.
#[test]
fn bytes_that_were_going_carry_on_going_after_a_restart() {
    let (mut sim, _) = Sim::group(1, 1);
    sim.import(PHONE, &bytes(14, 8_000_000));
    let id = imports(&sim, PHONE)[0].id;
    let outputs = block_on(sim.devices[PHONE].abandon_import(id)).expect("abandoned");
    assert!(outputs.unfinished);
    sim.absorb(PHONE, outputs);
    sim.restart(PHONE);
    sim.settle();

    assert_eq!(kept(&sim, PHONE), 0);
}
