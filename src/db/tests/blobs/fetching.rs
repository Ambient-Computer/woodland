//! Blobs between devices: asked for, checked, and served.

use super::*;
use crate::BlobPolicy;
use crate::blob::fetch::{Message, Part};
use crate::encoding;

fn policy(cluster: &mut Sim, device: usize, policy: BlobPolicy) {
    let outputs = block_on(cluster.devices[device].set_blob_policy(policy)).expect("said");
    cluster.absorb(device, outputs);
}

fn held(cluster: &Sim, device: usize, blob: &BlobRef) -> u64 {
    block_on(cluster.devices[device].blob_held(blob)).expect("counted")
}

fn fetch(cluster: &mut Sim, device: usize, blob: &BlobRef) {
    let outputs = block_on(cluster.devices[device].fetch_blob(blob)).expect("asked");
    cluster.absorb(device, outputs);
}

/// A table of pictures, with one the phone brought in.
fn pictured(cluster: &mut Sim, content: &[u8]) -> (ModelId, BlobRef) {
    let table = people(cluster, 1);
    create_with_pictures(cluster, PHONE, table);
    let blob = cluster.import(PHONE, content);
    insert_with(cluster, PHONE, table, "ada", blob);

    (table, blob)
}

#[test]
fn a_device_holds_no_group_until_it_says_what_it_fetches() {
    let opened = block_on(Database::open(MemoryStore::default(), secret(PHONE)));
    let mut device = opened.expect("opened");

    let founded = block_on(device.found([7; 32]));
    assert!(matches!(founded, Err(Error::NoBlobPolicy)));

    drop(block_on(device.set_blob_policy(BlobPolicy::All)).expect("said"));
    assert!(block_on(device.found([7; 32])).is_ok());
    assert_eq!(device.blob_policy(), Some(BlobPolicy::All));
}

#[test]
fn a_device_that_fetches_every_blob_gets_the_ones_it_reads_of() {
    let mut cluster = household();
    policy(&mut cluster, LAPTOP, BlobPolicy::All);
    for length in [0, 1, 16_384, 20_000, 114_688, 500_000] {
        let content = bytes(1, length);
        let table = people(&cluster, u8::try_from(length % 251).expect("small"));
        create_with_pictures(&mut cluster, PHONE, table);
        let blob = cluster.import(PHONE, &content);
        insert_with(&mut cluster, PHONE, table, "ada", blob);
        cluster.settle();

        assert_eq!(all(&cluster, LAPTOP, &blob), Some(content), "{length}");
        assert_eq!(held(&cluster, NAS, &blob), 0, "it fetches none");
    }
}

/// More than a page of hashes, so the pages' hashes come first.
#[test]
fn a_long_blob_is_fetched_whole() {
    let mut cluster = household();
    policy(&mut cluster, LAPTOP, BlobPolicy::All);
    let content = bytes(2, 2050 * 16_384 + 77);
    let (_, blob) = pictured(&mut cluster, &content);
    cluster.settle();

    assert_eq!(all(&cluster, LAPTOP, &blob), Some(content));
}

#[test]
fn a_policy_fetches_blobs_up_to_a_size() {
    let mut cluster = household();
    policy(&mut cluster, LAPTOP, BlobPolicy::UpTo(50_000));
    let table = people(&cluster, 1);
    create_with_pictures(&mut cluster, PHONE, table);
    let small = cluster.import(PHONE, &bytes(3, 50_000));
    let large = cluster.import(PHONE, &bytes(4, 50_001));
    insert_with(&mut cluster, PHONE, table, "ada", small);
    insert_with(&mut cluster, PHONE, table, "bob", large);
    cluster.settle();

    assert_eq!(held(&cluster, LAPTOP, &small), 50_000);
    assert_eq!(held(&cluster, LAPTOP, &large), 0);
}

#[test]
fn a_blob_the_policy_leaves_out_is_fetched_when_asked_for() {
    let mut cluster = household();
    let content = bytes(5, 300_000);
    let (_, blob) = pictured(&mut cluster, &content);
    cluster.settle();
    assert_eq!(held(&cluster, LAPTOP, &blob), 0);

    fetch(&mut cluster, LAPTOP, &blob);
    cluster.settle();

    assert_eq!(all(&cluster, LAPTOP, &blob), Some(content));
}

#[test]
fn a_policy_said_later_fetches_what_was_left_out() {
    let mut cluster = household();
    let content = bytes(6, 200_000);
    let (_, blob) = pictured(&mut cluster, &content);
    cluster.settle();

    policy(&mut cluster, NAS, BlobPolicy::All);
    cluster.settle();

    assert_eq!(all(&cluster, NAS, &blob), Some(content));
}

#[test]
fn a_model_may_be_fetched_differently() {
    let mut cluster = household();
    policy(&mut cluster, LAPTOP, BlobPolicy::All);
    let (table, wanted) = pictured(&mut cluster, &bytes(7, 40_000));
    let other = people(&cluster, 2);
    let outputs =
        block_on(cluster.devices[LAPTOP].set_model_blob_policy(other, Some(BlobPolicy::None)));
    cluster.absorb(LAPTOP, outputs.expect("said"));
    create_with_pictures(&mut cluster, PHONE, other);
    let unwanted = cluster.import(PHONE, &bytes(8, 40_000));
    insert_with(&mut cluster, PHONE, other, "bob", unwanted);
    cluster.settle();

    assert_eq!(held(&cluster, LAPTOP, &wanted), 40_000);
    assert_eq!(held(&cluster, LAPTOP, &unwanted), 0);
    assert!(read_table(&cluster, LAPTOP, table).is_some());
}

/// Knowing a name isn't enough.
#[test]
fn a_blob_is_served_only_within_a_group_that_refers_to_it() {
    let mut cluster = household();
    let blob = cluster.import(PHONE, &bytes(9, 30_000));
    cluster.settle();

    let asked = block_on(cluster.devices[LAPTOP].fetch_blob(&blob));
    assert!(matches!(asked, Err(Error::BlobUnreferred)));

    // Asked for anyway, by a peer that makes up what it likes.
    let ask = Message::Ask {
        group: cluster.home,
        blob: blob.listed(),
        part: Part::Piece(0),
    };
    let laptop = cluster.id(LAPTOP);
    let received = block_on(cluster.devices[PHONE].receive_blob(laptop, &encoding::encode(&ask)));
    drop(received.expect("received"));
    let answer = block_on(cluster.devices[PHONE].next_blob_message(laptop)).expect("taken");
    let answer = answer.and_then(|it| encoding::decode::<Message>(&it));

    assert!(matches!(answer, Some(Message::Lacks { .. })));
}

/// What arrives is checked against the name before it's stored.
#[test]
fn bytes_that_arent_the_blobs_are_dropped() {
    let mut cluster = household();
    let content = bytes(10, 100_000);
    let (_, blob) = pictured(&mut cluster, &content);
    cluster.settle();
    fetch(&mut cluster, LAPTOP, &blob);

    // The laptop asks, and is answered with bytes that are wrong.
    let phone = cluster.id(PHONE);
    let mut asked = 0;
    while let Some(ask) = block_on(cluster.devices[LAPTOP].next_blob_message(phone)).expect("taken")
    {
        let Some(Message::Ask { blob, part, .. }) = encoding::decode(&ask) else {
            panic!("it only has things to ask");
        };
        let length = match part {
            Part::Leaves(_) => 7 * 32,
            _ => 100_000,
        };
        let wrong = Message::Have {
            blob,
            part,
            bytes: vec![0xAB; length],
        };
        let received =
            block_on(cluster.devices[LAPTOP].receive_blob(phone, &encoding::encode(&wrong)));
        drop(received.expect("received"));
        asked += 1;
    }

    assert!(asked > 0);
    assert_eq!(held(&cluster, LAPTOP, &blob), 0);

    // Time passes, and it's asked for again, of a peer that's honest.
    cluster.blobs.clear();
    cluster.settle();
    assert_eq!(all(&cluster, LAPTOP, &blob), Some(content));
}

/// What has arrived isn't fetched again.
#[test]
fn a_fetch_that_stops_carries_on_from_where_it_was() {
    let mut cluster = household();
    let content = bytes(11, 3_000_000);
    let (_, blob) = pictured(&mut cluster, &content);
    cluster.settle();
    fetch(&mut cluster, LAPTOP, &blob);
    for _ in 0..40 {
        cluster.carry();
    }
    let partway = held(&cluster, LAPTOP, &blob);
    assert!(0 < partway && partway < blob.length(), "{partway}");

    cluster.isolate(LAPTOP);
    cluster.restart(LAPTOP);
    assert_eq!(held(&cluster, LAPTOP, &blob), partway);
    assert_eq!(
        read(&cluster, LAPTOP, &blob, 0, 10),
        Some(content[..10].to_vec())
    );
    assert_eq!(read(&cluster, LAPTOP, &blob, blob.length() - 10, 10), None);

    let before = cluster.parts[LAPTOP];
    cluster.connect_all();
    cluster.settle();

    assert_eq!(all(&cluster, LAPTOP, &blob), Some(content));
    let pieces = usize::try_from((blob.length() - partway).div_ceil(114_688)).expect("few");
    assert!(
        cluster.parts[LAPTOP] - before <= pieces + 2,
        "nothing twice"
    );
}

/// A device serves what it holds, whoever brought it in: so a blob
/// gets to a device that has never met its author.
#[test]
fn a_blob_passes_through_a_device_that_fetched_it() {
    let mut cluster = household();
    policy(&mut cluster, NAS, BlobPolicy::All);
    let content = bytes(12, 250_000);
    cluster.isolate(FRIEND);
    let (_, blob) = pictured(&mut cluster, &content);
    cluster.settle();
    cluster.isolate(PHONE);
    cluster.isolate(LAPTOP);
    cluster.connect(FRIEND, NAS);
    cluster.settle();

    fetch(&mut cluster, FRIEND, &blob);
    cluster.settle();

    assert_eq!(all(&cluster, FRIEND, &blob), Some(content));
}

/// Having fetched all of it, a device could hand it over, and so may
/// write a reference of its own.
#[test]
fn a_device_that_fetched_all_of_a_blob_may_refer_to_it() {
    let mut cluster = household();
    policy(&mut cluster, LAPTOP, BlobPolicy::All);
    let (table, blob) = pictured(&mut cluster, &bytes(13, 150_000));
    cluster.settle();

    let cells = [(picture(table), Value::from(blob))];
    cluster.write(LAPTOP, Transaction::new().insert(table, "bob", cells));
    cluster.settle();

    assert_eq!(
        read_table(&cluster, PHONE, table)
            .expect("a table")
            .rows()
            .len(),
        2
    );
}

/// The reference remains, and the bytes come back only when asked
/// for.
#[test]
fn a_blob_let_go_isnt_fetched_again_until_its_asked_for() {
    let mut cluster = household();
    policy(&mut cluster, LAPTOP, BlobPolicy::All);
    let content = bytes(14, 400_000);
    let (table, blob) = pictured(&mut cluster, &content);
    cluster.settle();
    assert_eq!(held(&cluster, LAPTOP, &blob), 400_000);

    let outputs = block_on(cluster.devices[LAPTOP].release_blob(&blob)).expect("let go");
    cluster.absorb(LAPTOP, outputs);
    cluster.settle();
    assert_eq!(held(&cluster, LAPTOP, &blob), 0);

    // Another reference to it changes nothing.
    insert_with(&mut cluster, PHONE, table, "bob", blob);
    cluster.settle();
    assert_eq!(held(&cluster, LAPTOP, &blob), 0);
    assert_eq!(
        read_table(&cluster, LAPTOP, table)
            .expect("a table")
            .rows()
            .len(),
        2
    );

    fetch(&mut cluster, LAPTOP, &blob);
    cluster.settle();
    assert_eq!(all(&cluster, LAPTOP, &blob), Some(content));
}

/// A reference whose only holder is away refers to nothing, for now.
#[test]
fn a_blob_nobody_reachable_holds_is_asked_for_again_later() {
    let mut cluster = household();
    policy(&mut cluster, FRIEND, BlobPolicy::All);
    let content = bytes(15, 60_000);
    cluster.isolate(PHONE);
    cluster.connect(PHONE, LAPTOP);
    let (_, blob) = pictured(&mut cluster, &content);
    cluster.settle();
    assert_eq!(
        held(&cluster, FRIEND, &blob),
        0,
        "the laptop has none to give"
    );

    fetch(&mut cluster, LAPTOP, &blob);
    for _ in 0..8 {
        cluster.settle();
    }

    assert_eq!(all(&cluster, FRIEND, &blob), Some(content));
}

/// A replaced value's blob isn't one a read would find.
#[test]
fn a_blob_only_a_replaced_value_refers_to_isnt_fetched() {
    let mut cluster = household();
    let (table, first) = pictured(&mut cluster, &bytes(16, 30_000));
    cluster.settle();
    let read = read_table(&cluster, PHONE, table).expect("a table");
    let row = read.rows().first().expect("a row");
    let second = cluster.import(PHONE, &bytes(17, 30_000));
    let cells = [(picture(table), Value::from(second))];
    cluster.write(PHONE, Transaction::new().update(table, row, cells));
    cluster.settle();

    policy(&mut cluster, LAPTOP, BlobPolicy::All);
    cluster.settle();

    assert_eq!(held(&cluster, LAPTOP, &first), 0);
    assert_eq!(held(&cluster, LAPTOP, &second), 30_000);
}
