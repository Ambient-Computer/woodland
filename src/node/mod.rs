mod batch;
mod hydration;
mod network;
mod outbox;
mod wire;

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

pub use batch::{Batch, TransactionBuilder};
use ed25519_dalek::VerifyingKey;
use hydration::Hydration;
use network::Network;
pub(crate) use outbox::Outbox;
use wire::Message;

use crate::actor::{self, Identity, Peer};
use crate::agreement::AgreementId;
use crate::encoding;
use crate::group::{Group, VerifiedParts};
use crate::io::{Action, Event, LocalEvent, NetworkEvent, StorageEvent};

/// Represents this device. Applications should only create one.
///
/// Manages current state, updates, and subscriptions.
///
/// A state machine that never performs I/O itself. Each [`Batch`] of
/// events moves it to a new state, and the [`Action`]s it returns are how
/// that state reaches the outside world. Performing them eventually
/// produces new events (a reply arrives, a peer drops), which go into the
/// next batch.
///
/// Processing a batch has three stages:
///
/// 1. **Apply** each event in order. Domains take in changes and queue
///    storage work in the `Outbox`. Nothing is sent yet.
/// 2. **Settle** each group: learn, integrate, report progress, compact.
/// 3. **Compose** one message per connected peer, and order everything:
///    storage first, then scans, then sends.
#[derive(Debug)]
pub struct Node {
    /// Who this device is. Every local change is signed with it.
    identity: Identity,
    network: Network,
    /// Every agreement this device replicates.
    groups: BTreeMap<AgreementId, Group>,
    hydration: Hydration,
}

impl Node {
    /// Starts a node for this device, holding nothing yet. Call
    /// [`Batch::hydrate`] to load what's in storage.
    #[must_use]
    pub fn new(identity: Identity) -> Self {
        Self {
            identity,
            network: Network::default(),
            groups: BTreeMap::new(),
            hydration: Hydration::default(),
        }
    }

    /// Starts a batch of events to process together.
    pub fn batch(&mut self) -> Batch<'_> {
        Batch::new(self)
    }

    /// The contributors in the latest configuration this device has learned.
    #[must_use]
    pub fn contributors(&self, agreement: AgreementId) -> Vec<VerifyingKey> {
        let Some(group) = self.groups.get(&agreement) else {
            return Vec::new();
        };

        let configuration = &group.agreement().learned().configuration;
        configuration.members().iter().map(Peer::id).collect()
    }

    fn me(&self) -> Peer {
        self.identity.peer()
    }

    fn process(&mut self, events: Vec<Event>) -> Vec<Action> {
        let mut outbox = Outbox::default();
        for event in events {
            self.apply(event, &mut outbox);
        }

        for group in self.groups.values_mut() {
            group.settle(&self.identity, &mut outbox);
        }

        let messages = self.compose();
        outbox.into_actions(messages)
    }

    fn apply(&mut self, event: Event, outbox: &mut Outbox) {
        match event {
            Event::Network(NetworkEvent::Connected { peer }) => {
                self.network.connect(peer);
            }

            Event::Network(NetworkEvent::Disconnected { peer }) => {
                self.network.disconnect(&peer);
                self.resubscribe();
            }

            Event::Network(NetworkEvent::Message { peer, bytes }) => {
                self.receive(peer, &bytes, outbox);
            }

            Event::Storage(StorageEvent::Hydrate) => hydration::start(outbox),

            Event::Storage(StorageEvent::Scanned { prefix, entries }) => {
                let me = self.me();
                let restored =
                    hydration::scanned(&mut self.hydration, me, &prefix, &entries, outbox);
                if let Some(group) = restored {
                    self.groups.insert(group.id(), group);
                    self.resubscribe();
                }
            }

            Event::Local(LocalEvent::Join { genesis }) => {
                let me = self.me();
                if let Entry::Vacant(vacant) = self.groups.entry(genesis.id()) {
                    vacant.insert(Group::join(&genesis, me, outbox));
                    self.resubscribe();
                }
            }

            Event::Local(LocalEvent::Reconfigure { agreement, change }) => {
                if let Some(group) = self.groups.get_mut(&agreement) {
                    group.propose(change);
                }
            }

            Event::Local(LocalEvent::Write {
                agreement,
                model,
                operations,
            }) => {
                if let Some(group) = self.groups.get_mut(&agreement) {
                    group.write(&self.identity, model, operations, outbox);
                }
            }
        }
    }

    /// Decodes and validates a peer's message, then hands each agreement its
    /// part. Every signature in the message is checked in one batch first.
    fn receive(&mut self, from: Peer, bytes: &[u8], outbox: &mut Outbox) {
        let Some(mut message) = encoding::decode::<Message>(bytes) else {
            tracing::warn!(peer = ?from, "dropped an undecodable message");
            return;
        };

        if let Some(replicates) = message.hello.take() {
            self.network.heard_hello(from, replicates);
            self.resubscribe();
        }

        // One batch per payload type. Sharing a single batch would need one
        // signed type for both, which isn't worth it for one fewer call.
        let (mut attestations, mut transactions) = (Vec::new(), Vec::new());
        for part in message.groups.values_mut() {
            attestations.append(&mut part.attestations);
            transactions.append(&mut part.transactions);
        }

        let mut verified: BTreeMap<AgreementId, VerifiedParts> = BTreeMap::new();
        for attestation in actor::verify_all(attestations) {
            let agreement = attestation.payload().agreement;
            let part = verified.entry(agreement).or_default();
            part.attestations.push(attestation);
        }

        for transaction in actor::verify_all(transactions) {
            let agreement = transaction.payload().agreement;
            let part = verified.entry(agreement).or_default();
            part.transactions.push(transaction);
        }

        for (id, part) in message.groups {
            if let Some(group) = self.groups.get_mut(&id) {
                let signed = verified.remove(&id).unwrap_or_default();
                group.receive(from, &part, signed, outbox);
            }
        }
    }

    /// Keeps each group's peer list equal to the connected peers that
    /// replicate it.
    fn resubscribe(&mut self) {
        for (&id, group) in &mut self.groups {
            group.subscribe_only(&self.network.subscribers(id));
        }
    }

    /// One message per connected peer: a hello if our agreements changed,
    /// plus whatever each agreement says the peer is missing.
    fn compose(&mut self) -> Vec<(Peer, Vec<u8>)> {
        let ours: BTreeSet<AgreementId> = self.groups.keys().copied().collect();
        let peers: Vec<Peer> = self.network.connected().collect();

        let mut messages = Vec::new();
        for peer in peers {
            let mut message = Message {
                hello: self.network.hello_for(peer, &ours),
                groups: BTreeMap::new(),
            };

            for (&id, group) in &mut self.groups {
                if let Some(part) = group.outgoing(peer) {
                    message.groups.insert(id, part);
                }
            }

            if message != Message::default() {
                messages.push((peer, encoding::encode(&message)));
            }
        }

        messages
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::agreement::Genesis;
    use crate::lattice::Lattice;
    use crate::model::{ModelId, Query};
    use crate::storage::keys::{self, Record};
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
            batch.transaction(agreement, model).set(key, value).commit()
        });
    }

    /// Reads straight from the device's storage, as an application would.
    fn read(cluster: &Cluster, device: usize, key: &str) -> Option<String> {
        let query = Query::key(genesis(cluster).id(), notes(cluster), key.as_bytes());
        let scanned = cluster.devices[device].store.scan(query.storage_prefix());
        let (_, value) = query.results(&scanned).pop()?;

        Some(String::from_utf8_lossy(&value).into_owned())
    }

    fn group(cluster: &Cluster, device: usize) -> &Group {
        &cluster.devices[device].node.groups[&genesis(cluster).id()]
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

        // Signed and delivered, but the friend isn't a contributor yet, so
        // nobody integrates it. Not even the friend.
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

    #[test]
    fn quiet_contributors_compact_everything() {
        let mut cluster = household();
        write(&mut cluster, PHONE, "color", "blue");
        write(&mut cluster, LAPTOP, "size", "large");
        cluster.settle();

        for device in [PHONE, LAPTOP, NAS] {
            assert_eq!(stored_log_len(&cluster, device), 0, "device {device}");
            assert_eq!(
                group(&cluster, device)
                    .history()
                    .floor()
                    .get(cluster.key(PHONE).into()),
                1
            );
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

        // Learned and compacted, which takes a quorum of contributors.
        for device in [PHONE, LAPTOP, NAS] {
            assert_eq!(stored_log_len(&cluster, device), 0, "device {device}");
        }
    }

    #[test]
    fn steady_writing_still_compacts() {
        let mut cluster = household();
        for round in 0..40 {
            for device in [PHONE, LAPTOP, NAS] {
                write(&mut cluster, device, "counter", &round.to_string());
                cluster.deliver(device);
            }
        }

        // Never quiet, yet the floor keeps moving.
        let phone = cluster.key(PHONE).into();
        let floor = group(&cluster, LAPTOP).history().floor().get(phone);
        assert!(floor > 20, "floor stuck at {floor}");
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

        assert_eq!(read(&cluster, FRIEND, "color").as_deref(), Some("blue"));
    }

    #[test]
    fn a_restarted_device_picks_up_where_it_left_off() {
        let mut cluster = household();
        write(&mut cluster, PHONE, "color", "blue");
        cluster.settle();
        let before = group(&cluster, PHONE).models().clone();
        let learned = group(&cluster, PHONE).agreement().learned().clone();
        let attested = group(&cluster, PHONE).agreement().attested().cloned();

        cluster.restart(PHONE);

        assert_eq!(group(&cluster, PHONE).models(), &before);
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

    #[derive(Clone, Copy, Debug)]
    enum Step {
        Write {
            device: usize,
            key: u8,
            value: u8,
        },
        Connect {
            left: usize,
            right: usize,
        },
        Disconnect {
            left: usize,
            right: usize,
        },
        /// One device processes everything waiting for it.
        Deliver {
            device: usize,
        },
        Restart {
            device: usize,
        },
        AddFriend {
            device: usize,
        },
        RemoveNas {
            device: usize,
        },
    }

    fn step() -> impl Strategy<Value = Step> {
        let device = 0..4usize;
        prop_oneof![
            4 => (device.clone(), 0..3u8, any::<u8>())
                .prop_map(|(device, key, value)| Step::Write { device, key, value }),
            2 => (device.clone(), device.clone())
                .prop_map(|(left, right)| Step::Connect { left, right }),
            2 => (device.clone(), device.clone())
                .prop_map(|(left, right)| Step::Disconnect { left, right }),
            6 => device.clone().prop_map(|device| Step::Deliver { device }),
            1 => device.clone().prop_map(|device| Step::Restart { device }),
            1 => (0..3usize).prop_map(|device| Step::AddFriend { device }),
            1 => (0..3usize).prop_map(|device| Step::RemoveNas { device }),
        ]
    }

    fn run(cluster: &mut Cluster, step: Step) {
        let agreement = genesis(cluster).id();
        match step {
            Step::Write { device, key, value } => {
                write(cluster, device, &key.to_string(), &value.to_string());
            }
            Step::Connect { left, right } => cluster.connect(left, right),
            Step::Disconnect { left, right } => cluster.disconnect(left, right),
            Step::Deliver { device } => cluster.deliver(device),
            Step::Restart { device } => cluster.restart(device),
            Step::AddFriend { device } => {
                let friend = cluster.key(FRIEND);
                cluster.run(device, |batch| batch.add_contributor(agreement, friend));
            }
            Step::RemoveNas { device } => {
                let nas = cluster.key(NAS);
                cluster.run(device, |batch| batch.remove_contributor(agreement, nas));
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        /// Whatever happened (partitions, crashes, concurrent writes and
        /// reconfigurations), once everyone reconnects they agree on
        /// everything: data, membership, and what storage holds.
        #[test]
        fn reconnected_devices_converge(steps in prop::collection::vec(step(), 0..60)) {
            let mut cluster = household();
            let mut learned = Vec::new();
            for step in steps {
                run(&mut cluster, step);
                for device in [PHONE, LAPTOP, NAS, FRIEND] {
                    learned.push(group(&cluster, device).agreement().learned().clone());
                }
            }

            cluster.connect_everyone();
            cluster.settle();

            // Lattice agreement's promise holds end to end, through real
            // sync, storage and restarts: no two devices ever learned
            // decisions that disagree.
            for left in &learned {
                for right in &learned {
                    prop_assert!(left.leq(right) || right.leq(left));
                }
            }

            let reference = group(&cluster, PHONE);
            for device in [LAPTOP, NAS, FRIEND] {
                let other = group(&cluster, device);
                prop_assert_eq!(other.models(), reference.models(), "device {}", device);
                prop_assert_eq!(
                    other.agreement().learned(),
                    reference.agreement().learned(),
                    "device {}", device,
                );
            }

            // Compaction really forgets: nothing at or below the floor stays
            // in storage.
            for device in [PHONE, LAPTOP, NAS, FRIEND] {
                let floor = group(&cluster, device).history().floor().clone();
                for key in cluster.devices[device].store.keys() {
                    if let Some(Record::Log { author, seq, .. }) = keys::parse(key) {
                        let author = VerifyingKey::from_bytes(&author).map(Peer::from);
                        prop_assert!(author.is_ok_and(|author| !floor.contains(author, seq)));
                    }
                }
            }

            // Storage holds exactly what memory does.
            for device in [PHONE, LAPTOP, NAS, FRIEND] {
                let models = group(&cluster, device).models().clone();
                cluster.restart(device);
                prop_assert_eq!(group(&cluster, device).models(), &models, "device {}", device);
            }
        }
    }
}
