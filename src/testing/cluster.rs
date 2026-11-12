//! A cluster of devices with in-memory storage and an in-memory network.
//!
//! This plays the application's role exactly as the public API asks: it
//! feeds events into batches and performs the actions that come out, in
//! order. Tests drive nodes the same way a real application would, so
//! nothing here needs private access to the library.

use std::collections::{BTreeSet, VecDeque};

use ed25519_dalek::VerifyingKey;

use super::peers;
use super::store::MemoryStore;
use crate::actor::Identity;
use crate::io::{Action, Changed, NetworkAction};
use crate::node::{Batch, Node};
use crate::storage::StorageAction;

/// The cast, by index.
pub(crate) const PHONE: usize = 0;
pub(crate) const LAPTOP: usize = 1;
pub(crate) const NAS: usize = 2;
pub(crate) const FRIEND: usize = 3;

pub(crate) struct Device {
    pub(crate) node: Node,
    pub(crate) store: MemoryStore,
    key: VerifyingKey,
    /// Every change the node has reported, oldest first.
    pub(crate) changes: Vec<Changed>,
    /// Events waiting to go into this device's next batch.
    inbox: VecDeque<Incoming>,
}

enum Incoming {
    Connected(VerifyingKey),
    Disconnected(VerifyingKey),
    Message(VerifyingKey, Vec<u8>),
    Scanned(Vec<u8>, Vec<(Vec<u8>, Vec<u8>)>),
    Got(Vec<(Vec<u8>, Option<Vec<u8>>)>),
}

pub(crate) struct Cluster {
    pub(crate) devices: Vec<Device>,
    /// Open connections, as (lower index, higher index).
    links: BTreeSet<(usize, usize)>,
}

fn identity(index: usize) -> Identity {
    [
        peers::phone_identity,
        peers::laptop_identity,
        peers::nas_identity,
        peers::friend_identity,
    ][index]()
}

impl Cluster {
    /// Phone, laptop, nas and friend, all offline, holding nothing.
    pub(crate) fn new() -> Self {
        let devices = (0..4)
            .map(|index| Device {
                node: Node::new(identity(index)),
                store: MemoryStore::default(),
                key: identity(index).public_key(),
                changes: Vec::new(),
                inbox: VecDeque::new(),
            })
            .collect();

        Self {
            devices,
            links: BTreeSet::new(),
        }
    }

    pub(crate) fn key(&self, device: usize) -> VerifyingKey {
        self.devices[device].key
    }

    /// Runs a batch built by `build` on one device, and performs its actions.
    pub(crate) fn run(&mut self, device: usize, build: impl FnOnce(Batch<'_>) -> Batch<'_>) {
        let actions = build(self.devices[device].node.batch()).process();
        self.perform(device, actions);
    }

    pub(crate) fn connect(&mut self, left: usize, right: usize) {
        if left != right && self.links.insert(link(left, right)) {
            let (left_key, right_key) = (self.key(left), self.key(right));
            self.devices[left]
                .inbox
                .push_back(Incoming::Connected(right_key));
            self.devices[right]
                .inbox
                .push_back(Incoming::Connected(left_key));
        }
    }

    pub(crate) fn disconnect(&mut self, left: usize, right: usize) {
        if self.links.remove(&link(left, right)) {
            let (left_key, right_key) = (self.key(left), self.key(right));
            self.devices[left]
                .inbox
                .push_back(Incoming::Disconnected(right_key));
            self.devices[right]
                .inbox
                .push_back(Incoming::Disconnected(left_key));
        }
    }

    pub(crate) fn connect_everyone(&mut self) {
        for left in 0..self.devices.len() {
            for right in left + 1..self.devices.len() {
                self.connect(left, right);
            }
        }
    }

    pub(crate) fn disconnect_everyone(&mut self) {
        for (left, right) in self.links.clone() {
            self.disconnect(left, right);
        }
    }

    /// Simulates a crash: the node's memory is gone, its storage remains.
    /// The device comes back offline and hydrates from storage.
    ///
    /// Messages already in flight to it are lost with its memory.
    pub(crate) fn restart(&mut self, device: usize) {
        for other in 0..self.devices.len() {
            self.disconnect(device, other);
        }

        self.devices[device].node = Node::new(identity(device));
        self.devices[device].inbox.clear();
        let actions = self.devices[device].node.batch().hydrate().process();
        self.perform(device, actions);
        while !self.devices[device].inbox.is_empty() {
            self.deliver(device);
        }
    }

    /// Processes one device's whole inbox as one batch.
    pub(crate) fn deliver(&mut self, device: usize) {
        let incoming: Vec<Incoming> = self.devices[device].inbox.drain(..).collect();
        self.run(device, |mut batch| {
            for event in incoming {
                batch = match event {
                    Incoming::Connected(peer) => batch.connected(peer),
                    Incoming::Disconnected(peer) => batch.disconnected(peer),
                    Incoming::Message(peer, bytes) => batch.message(peer, bytes),
                    Incoming::Scanned(prefix, entries) => batch.scanned(prefix, entries),
                    Incoming::Got(entries) => batch.got(entries),
                };
            }

            batch
        });
    }

    /// Delivers everything in flight, batch by batch, until nothing is left.
    /// Each device processes its whole inbox as one batch.
    pub(crate) fn settle(&mut self) {
        for _ in 0..1_000 {
            let Some(device) = self.devices.iter().position(|it| !it.inbox.is_empty()) else {
                return;
            };

            self.deliver(device);
        }

        panic!("the cluster never went quiet");
    }

    /// Plays the application: performs actions strictly in order.
    fn perform(&mut self, device: usize, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Storage(StorageAction::Write(writes)) => {
                    self.devices[device].store.write(&writes);
                }

                Action::Storage(StorageAction::Scan { prefix }) => {
                    let entries = self.devices[device].store.scan(&prefix);
                    let scanned = Incoming::Scanned(prefix, entries);
                    self.devices[device].inbox.push_back(scanned);
                }

                Action::Storage(StorageAction::Get { keys }) => {
                    let entries = self.devices[device].store.get(keys);
                    self.devices[device].inbox.push_back(Incoming::Got(entries));
                }

                Action::Changed(change) => self.devices[device].changes.push(change),

                Action::Network(NetworkAction::Send { peer, message }) => {
                    let sender = self.key(device);
                    let recipient = self.devices.iter().position(|it| it.key == peer);
                    if let Some(recipient) = recipient
                        && self.links.contains(&link(device, recipient))
                    {
                        let incoming = Incoming::Message(sender, message);
                        self.devices[recipient].inbox.push_back(incoming);
                    }
                }
            }
        }
    }
}

fn link(left: usize, right: usize) -> (usize, usize) {
    (left.min(right), left.max(right))
}
