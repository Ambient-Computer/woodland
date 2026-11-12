mod convergence;

use super::*;
use crate::agreement::Genesis;
use crate::model::{ModelId, Query};
use crate::storage::keys::Record;
use crate::testing::cluster::{Cluster, FRIEND, LAPTOP, NAS, PHONE};

/// The household's shared agreement. The friend can be invited to it.
fn genesis(cluster: &Cluster) -> Genesis {
    let founders = [PHONE, LAPTOP, NAS].map(|device| cluster.key(device));
    Genesis::new([7; 32], founders)
}

fn notes(cluster: &Cluster) -> ModelId {
    ModelId::new(genesis(cluster).id(), [1; 32])
}

/// Everyone, the friend included, holds the genesis and is online.
fn household() -> Cluster {
    let mut cluster = Cluster::new();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let genesis = genesis(&cluster);
        cluster.run(device, |batch| batch.join(genesis));
    }

    cluster.connect_everyone();
    cluster.settle();
    cluster
}

fn write(cluster: &mut Cluster, device: usize, key: &str, value: &str) {
    let (agreement, model) = (genesis(cluster).id(), notes(cluster));
    let (key, value) = (key.to_owned(), value.to_owned());
    cluster.run(device, |batch| {
        batch.transaction(agreement).set(model, key, value).commit()
    });
}

/// Reads straight from the device's storage, as an application would:
/// the most recent value the device knows of.
fn read(cluster: &Cluster, device: usize, key: &str) -> Option<String> {
    let query = Query::key(genesis(cluster).id(), notes(cluster), key.as_bytes());

    run_query(cluster, device, &query)
}

/// Reads only what has settled on the device.
fn read_committed(cluster: &Cluster, device: usize, key: &str) -> Option<String> {
    let query = Query::key(genesis(cluster).id(), notes(cluster), key.as_bytes());

    run_query(cluster, device, &query.committed())
}

fn run_query(cluster: &Cluster, device: usize, query: &Query) -> Option<String> {
    let store = &cluster.devices[device].store;
    let prefixes = query.storage_prefixes();
    let scanned: Vec<_> = prefixes.flat_map(|prefix| store.scan(prefix)).collect();
    let (_, value) = query.results(&scanned).pop()?;

    Some(String::from_utf8_lossy(&value).into_owned())
}

fn group(cluster: &Cluster, device: usize) -> &Group {
    &cluster.devices[device].node.groups[&genesis(cluster).id()]
}

/// Everything settled in a device's storage, as stored: the cut, and
/// every settled value.
fn stored_settled(cluster: &Cluster, device: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
    let store = &cluster.devices[device].store;
    store.scan(&keys::settled(genesis(cluster).id()))
}

/// Every version in a device's storage that hasn't settled, as stored.
fn stored_frontier(cluster: &Cluster, device: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
    let store = &cluster.devices[device].store;
    store.scan(&keys::frontier(genesis(cluster).id()))
}

/// Transactions still kept individually in a device's storage.
fn stored_log_len(cluster: &Cluster, device: usize) -> usize {
    let store = &cluster.devices[device].store;
    store
        .keys()
        .filter(|key| matches!(keys::parse(key), Some(Record::Log { .. })))
        .count()
}

#[test]
fn a_write_reaches_everyone() {
    let mut cluster = household();
    write(&mut cluster, PHONE, "color", "blue");
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(read(&cluster, device, "color").as_deref(), Some("blue"));
    }
}

#[test]
fn offline_writes_merge_when_devices_reconnect() {
    let mut cluster = household();
    cluster.disconnect_everyone();
    cluster.settle();

    write(&mut cluster, PHONE, "color", "blue");
    write(&mut cluster, LAPTOP, "color", "red");
    write(&mut cluster, LAPTOP, "size", "large");
    cluster.connect_everyone();
    cluster.settle();

    let winner = read(&cluster, PHONE, "color");
    for device in [LAPTOP, NAS, FRIEND] {
        assert_eq!(read(&cluster, device, "color"), winner);
        assert_eq!(read(&cluster, device, "size").as_deref(), Some("large"));
    }
}

#[test]
fn the_friend_can_write_once_added() {
    let mut cluster = household();
    write(&mut cluster, FRIEND, "gift", "book");
    cluster.settle();

    // The friend isn't a contributor yet, so the write is held on its
    // device, unsigned. Nobody sees it. Not even the friend.
    assert_eq!(read(&cluster, PHONE, "gift"), None);
    assert_eq!(read(&cluster, FRIEND, "gift"), None);

    let (agreement, friend) = (genesis(&cluster).id(), cluster.key(FRIEND));
    cluster.run(PHONE, |batch| batch.add_contributor(agreement, friend));
    cluster.settle();

    assert!(
        cluster.devices[NAS]
            .node
            .contributors(agreement)
            .contains(&friend)
    );
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(read(&cluster, device, "gift").as_deref(), Some("book"));
    }
}

/// A write made before admission isn't signed until the newcomer has
/// caught up, so it lands on top of what it caught up to. Signed when it
/// was made, it would have been written on top of nothing, and lost to
/// the phone's write.
#[test]
fn a_held_write_lands_on_top_of_what_the_newcomer_caught_up_to() {
    let mut cluster = household();
    write(&mut cluster, FRIEND, "color", "green");
    write(&mut cluster, PHONE, "color", "blue");
    write(&mut cluster, PHONE, "color", "navy");
    cluster.settle();

    let (agreement, friend) = (genesis(&cluster).id(), cluster.key(FRIEND));
    cluster.run(PHONE, |batch| batch.add_contributor(agreement, friend));
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(read(&cluster, device, "color").as_deref(), Some("green"));
    }
}

#[test]
fn quiet_contributors_settle_everything() {
    let mut cluster = household();
    write(&mut cluster, PHONE, "color", "blue");
    write(&mut cluster, LAPTOP, "size", "large");
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(stored_log_len(&cluster, device), 0, "device {device}");
        assert_eq!(
            group(&cluster, device)
                .log()
                .settled()
                .get(cluster.key(PHONE).into()),
            1
        );
    }
}

/// One transaction changes two models. Each keeps its own keys, and
/// neither change shows up without the other.
#[test]
fn a_transaction_spans_models() {
    let mut cluster = household();
    let agreement = genesis(&cluster).id();
    let (notes, tags) = (notes(&cluster), ModelId::new(agreement, [2; 32]));
    cluster.run(PHONE, |batch| {
        let transaction = batch.transaction(agreement);

        transaction
            .set(notes, "color", "blue")
            .set(tags, "color", "cool")
            .commit()
    });
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let tag = run_query(&cluster, device, &Query::key(agreement, tags, b"color"));

        assert_eq!(read(&cluster, device, "color").as_deref(), Some("blue"));
        assert_eq!(tag.as_deref(), Some("cool"));
    }
}

/// Peers don't form a full mesh. Transactions are signed by their
/// authors, so any device can relay them, and views merge, so they
/// spread hop by hop.
#[test]
fn a_chain_of_connections_is_enough() {
    let mut cluster = household();
    cluster.disconnect_everyone();
    cluster.connect(PHONE, NAS);
    cluster.connect(NAS, LAPTOP);
    cluster.connect(LAPTOP, FRIEND);
    cluster.settle();

    write(&mut cluster, PHONE, "color", "blue");
    write(&mut cluster, FRIEND, "gift", "book");
    let (agreement, friend) = (genesis(&cluster).id(), cluster.key(FRIEND));
    cluster.run(PHONE, |batch| batch.add_contributor(agreement, friend));
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(read(&cluster, device, "color").as_deref(), Some("blue"));
        assert_eq!(read(&cluster, device, "gift").as_deref(), Some("book"));
    }
}

/// The household's devices never meet. They only ever connect to the
/// friend, who isn't a contributor. No contributor hears another
/// directly, yet they all learn.
///
/// Learning takes a quorum of contributors holding the learner's view,
/// but the learner can be anyone. The friend hears all three, learns,
/// and the decision spreads back out like the rest of the view. With a
/// handful of devices, some device nearly always hears a quorum
/// directly.
#[test]
fn a_hub_that_isnt_a_contributor_learns_for_everyone() {
    let mut cluster = household();
    cluster.disconnect_everyone();
    for device in [PHONE, LAPTOP, NAS] {
        cluster.connect(device, FRIEND);
    }

    write(&mut cluster, PHONE, "color", "blue");
    cluster.settle();

    // Learned and settled, which takes a quorum of contributors.
    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(stored_log_len(&cluster, device), 0, "device {device}");
    }
}

#[test]
fn steady_writing_still_settles() {
    let mut cluster = household();
    for round in 0..40 {
        for device in [PHONE, LAPTOP, NAS] {
            write(&mut cluster, device, "counter", &round.to_string());
            cluster.deliver(device);
        }
    }

    // Never quiet, yet more keeps settling.
    let phone = cluster.key(PHONE).into();
    let settled = group(&cluster, LAPTOP).log().settled().get(phone);
    assert!(settled > 20, "settled stuck at {settled}");
}

#[test]
fn an_offline_contributor_holds_the_floor() {
    let mut cluster = household();
    cluster.disconnect(NAS, PHONE);
    cluster.disconnect(NAS, LAPTOP);
    cluster.disconnect(NAS, FRIEND);
    cluster.settle();

    write(&mut cluster, PHONE, "color", "blue");
    cluster.settle();

    // The phone and laptop are a quorum, so they can learn. But the
    // floor is a lower bound over every contributor, and the nas hasn't
    // reported seeing the write.
    assert_eq!(stored_log_len(&cluster, PHONE), 1);
}

/// A committed read only shows what every contributor is known to hold.
/// The optimistic read doesn't wait.
#[test]
fn a_committed_read_waits_for_every_contributor() {
    let mut cluster = household();
    for other in [PHONE, LAPTOP, FRIEND] {
        cluster.disconnect(NAS, other);
    }

    write(&mut cluster, PHONE, "color", "blue");
    cluster.settle();
    assert_eq!(read(&cluster, LAPTOP, "color").as_deref(), Some("blue"));
    assert_eq!(read_committed(&cluster, LAPTOP, "color"), None);

    cluster.connect(NAS, PHONE);
    cluster.settle();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let committed = read_committed(&cluster, device, "color");
        assert_eq!(committed.as_deref(), Some("blue"), "device {device}");
        assert_eq!(stored_frontier(&cluster, device), [], "device {device}");
    }
}

/// A settled removal leaves nothing in storage: no value, and no
/// tombstone either.
#[test]
fn a_settled_removal_leaves_nothing_behind() {
    let mut cluster = household();
    write(&mut cluster, PHONE, "color", "blue");
    cluster.settle();

    let (agreement, model) = (genesis(&cluster).id(), notes(&cluster));
    cluster.run(PHONE, |batch| {
        batch.transaction(agreement).remove(model, "color").commit()
    });
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let stored = stored_settled(&cluster, device);
        let values = stored
            .iter()
            .filter(|(key, _)| matches!(keys::parse(key), Some(Record::Settled { .. })));

        assert_eq!(values.count(), 0, "device {device}");
        assert_eq!(stored_frontier(&cluster, device), [], "device {device}");
    }
}

/// What a removed device wrote counts only if a contributor took it
/// before learning of the removal. The rest is discarded everywhere,
/// the removed device included.
#[test]
fn a_removed_devices_unseen_writes_are_discarded() {
    let mut cluster = household();
    write(&mut cluster, NAS, "seen", "kept");
    cluster.settle();

    for other in [PHONE, LAPTOP, FRIEND] {
        cluster.disconnect(NAS, other);
    }

    write(&mut cluster, NAS, "unseen", "lost");
    let (agreement, nas) = (genesis(&cluster).id(), cluster.key(NAS));
    cluster.run(LAPTOP, |batch| batch.remove_contributor(agreement, nas));
    cluster.settle();

    cluster.connect_everyone();
    cluster.settle();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(read(&cluster, device, "seen").as_deref(), Some("kept"));
        assert_eq!(read(&cluster, device, "unseen"), None, "device {device}");
    }
}

#[test]
fn removing_a_lost_device_releases_the_floor() {
    let mut cluster = household();
    for other in [PHONE, LAPTOP, FRIEND] {
        cluster.disconnect(NAS, other);
    }

    let (agreement, nas) = (genesis(&cluster).id(), cluster.key(NAS));
    cluster.run(LAPTOP, |batch| batch.remove_contributor(agreement, nas));
    write(&mut cluster, PHONE, "color", "blue");
    cluster.settle();

    assert!(
        !cluster.devices[PHONE]
            .node
            .contributors(agreement)
            .contains(&nas)
    );
    assert_eq!(stored_log_len(&cluster, PHONE), 0);
}

#[test]
fn a_late_device_catches_up_from_a_snapshot() {
    let mut cluster = household();
    cluster.disconnect(FRIEND, PHONE);
    cluster.disconnect(FRIEND, LAPTOP);
    cluster.disconnect(FRIEND, NAS);
    write(&mut cluster, PHONE, "color", "blue");
    cluster.settle();
    assert_eq!(stored_log_len(&cluster, PHONE), 0);

    // The transaction is gone from every log, so only a snapshot can
    // bring the friend up to date.
    cluster.connect(FRIEND, NAS);
    cluster.settle();

    let committed = read_committed(&cluster, FRIEND, "color");
    assert_eq!(committed.as_deref(), Some("blue"));
}

#[test]
fn a_restarted_device_picks_up_where_it_left_off() {
    let mut cluster = household();
    write(&mut cluster, PHONE, "color", "blue");
    cluster.settle();
    let clock = group(&cluster, PHONE).log().clock().clone();
    let learned = group(&cluster, PHONE).agreement().learned().clone();
    let attested = group(&cluster, PHONE).agreement().attested().cloned();

    cluster.restart(PHONE);

    // Protocol state comes back. Model data stays in storage.
    assert_eq!(group(&cluster, PHONE).log().clock(), &clock);
    assert_eq!(group(&cluster, PHONE).agreement().learned(), &learned);
    assert_eq!(
        group(&cluster, PHONE).agreement().attested(),
        attested.as_ref()
    );

    cluster.connect_everyone();
    write(&mut cluster, PHONE, "size", "small");
    cluster.settle();
    assert_eq!(read(&cluster, LAPTOP, "size").as_deref(), Some("small"));
}

/// Integrating never reads. After a restart nothing is in memory, and
/// an older write arrives for a key that already has a newer value in
/// storage. It's written anyway, behind the newer one in the timeline.
#[test]
fn an_older_write_still_loses_after_a_restart() {
    let mut cluster = household();
    cluster.disconnect_everyone();
    write(&mut cluster, NAS, "color", "old");
    write(&mut cluster, PHONE, "color", "draft");
    write(&mut cluster, PHONE, "color", "new");
    cluster.restart(PHONE);

    cluster.connect(PHONE, NAS);
    cluster.settle();

    assert_eq!(read(&cluster, PHONE, "color").as_deref(), Some("new"));
    assert_eq!(read(&cluster, NAS, "color").as_deref(), Some("new"));
}

#[test]
fn local_writes_survive_a_restart_before_write_access() {
    let mut cluster = household();
    cluster.disconnect_everyone();
    write(&mut cluster, FRIEND, "gift", "book");
    cluster.restart(FRIEND);

    let (agreement, friend) = (genesis(&cluster).id(), cluster.key(FRIEND));
    cluster.connect_everyone();
    cluster.run(PHONE, |batch| batch.add_contributor(agreement, friend));
    cluster.settle();

    assert_eq!(read(&cluster, LAPTOP, "gift").as_deref(), Some("book"));
}
