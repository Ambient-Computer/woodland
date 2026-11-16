//! A simulated network of devices, each with its own store.
//!
//! The simulation plays the application: it delivers what each step
//! asks to send, in an order the test controls, and nothing else moves.

mod allowances;
mod checkpoints;
mod clocks;
mod compaction;
mod conflicts;
mod foreign;
mod forks;
mod guards;
mod interest;
mod joining;
mod leaving;
mod membership;
mod random;
mod roles;
mod sealed;
mod sequences;
mod single;
mod sync;
mod tables;

use std::collections::{BTreeSet, VecDeque};

use ed25519_dalek::VerifyingKey;

use crate::db::{Changed, ClockId, GroupId, keys};
use crate::model::{ModelId, Query};
use crate::store::{MemoryStore, Store, Write};
use crate::testing::block_on;

use super::{Database, Error, Outputs, Transaction};
use crate::chain::Role;
use crate::seal::SealingKey;

const PHONE: usize = 0;
const LAPTOP: usize = 1;
const NAS: usize = 2;
const FRIEND: usize = 3;

struct Sim {
    devices: Vec<Database<MemoryStore>>,
    /// Messages on their way: from, to, bytes.
    queue: VecDeque<(usize, usize, Vec<u8>)>,
    links: BTreeSet<(usize, usize)>,
    changes: Vec<Vec<Changed>>,
    /// The group most tests are about.
    home: GroupId,
    /// The devices running a version of the library from before
    /// allowances were added.
    old: BTreeSet<usize>,
}

/// One device's view of what's stored.
struct Reader<'a>(&'a Sim, usize);

impl Reader<'_> {
    fn read(&self, query: &Query) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.0.read(self.1, query)
    }
}

fn secret(device: usize) -> [u8; 32] {
    [u8::try_from(device + 1).expect("a handful of devices"); 32]
}

impl Sim {
    fn new(devices: usize) -> Self {
        let open = |device| block_on(Database::open(MemoryStore::default(), secret(device)));
        let devices: Result<Vec<_>, Error> = (0..devices).map(open).collect();

        Self {
            devices: devices.expect("memory doesn't fail"),
            queue: VecDeque::new(),
            links: BTreeSet::new(),
            changes: vec![Vec::new(); 8],
            home: GroupId::from_bytes([0; 32]),
            old: BTreeSet::new(),
        }
    }

    /// A group founded by the phone and held by everyone, with the first
    /// `contributors` devices as equal contributors, all connected.
    fn group(devices: usize, contributors: usize) -> (Self, GroupId) {
        let mut sim = Self::new(devices);
        let (group, outputs) = block_on(sim.devices[PHONE].found([7; 32])).expect("founded");
        sim.absorb(PHONE, outputs);
        let invitation = sim.devices[PHONE].invite(group).expect("held");
        for device in 1..devices {
            let (_, outputs) = block_on(sim.devices[device].join(&invitation)).expect("joined");
            sim.absorb(device, outputs);
        }

        sim.connect_all();
        sim.reconfigure(PHONE, group, 0..contributors);
        sim.settle();
        sim.home = group;

        (sim, group)
    }

    /// Runs what comes next as the version of the library `device`
    /// has.
    fn wake(&self, device: usize) {
        crate::model::predate(self.old.contains(&device));
    }

    fn id(&self, device: usize) -> VerifyingKey {
        self.devices[device].id()
    }

    fn index(&self, id: VerifyingKey) -> usize {
        let found = self.devices.iter().position(|device| device.id() == id);

        found.expect("a message for a device in the simulation")
    }

    fn absorb(&mut self, device: usize, outputs: Outputs) {
        self.changes[device].extend(outputs.changed);
        for (to, message) in outputs.sends {
            let to = self.index(to);
            if self.linked(device, to) {
                self.queue.push_back((device, to, message));
            }
        }
    }

    fn linked(&self, a: usize, b: usize) -> bool {
        self.links.contains(&(a.min(b), a.max(b)))
    }

    fn connect(&mut self, a: usize, b: usize) {
        if !self.links.insert((a.min(b), a.max(b))) {
            return;
        }

        for (here, there) in [(a, b), (b, a)] {
            let there = self.id(there);
            self.wake(here);
            let outputs = block_on(self.devices[here].connected(there)).expect("connected");
            self.absorb(here, outputs);
        }
    }

    fn connect_all(&mut self) {
        for a in 0..self.devices.len() {
            for b in a + 1..self.devices.len() {
                self.connect(a, b);
            }
        }
    }

    /// Whatever was on its way between them is lost.
    fn disconnect(&mut self, a: usize, b: usize) {
        if !self.links.remove(&(a.min(b), a.max(b))) {
            return;
        }

        let (id_a, id_b) = (self.id(a), self.id(b));
        self.devices[a].disconnected(id_b);
        self.devices[b].disconnected(id_a);
        self.queue
            .retain(|&(from, to, _)| (from.min(to), from.max(to)) != (a.min(b), a.max(b)));
    }

    fn disconnect_all(&mut self) {
        for (a, b) in self.links.clone() {
            self.disconnect(a, b);
        }
    }

    fn isolate(&mut self, device: usize) {
        for other in 0..self.devices.len() {
            self.disconnect(device, other);
        }
    }

    /// Delivers the message that has waited longest. Returns whether
    /// there was one.
    fn deliver(&mut self) -> bool {
        let Some((from, to, message)) = self.queue.pop_front() else {
            return false;
        };

        let from = self.id(from);
        self.wake(to);
        let outputs = block_on(self.devices[to].receive(from, &message)).expect("received");
        self.absorb(to, outputs);

        true
    }

    /// Delivers everything, and everything that follows from it.
    fn run(&mut self) {
        let mut delivered = 0;
        while self.deliver() || self.resume() {
            delivered += 1;
            assert!(delivered < 100_000, "the devices never stop talking");
        }
    }

    /// Has the first device with something left to do carry on with
    /// it. Returns whether there was one.
    fn resume(&mut self) -> bool {
        let busy = |device: &Database<MemoryStore>| {
            device.groups.values().any(super::group::Group::is_busy)
        };
        let Some(device) = self.devices.iter().position(busy) else {
            return false;
        };

        self.wake(device);
        let outputs = block_on(self.devices[device].resume()).expect("resumed");
        self.absorb(device, outputs);

        true
    }

    fn tick(&mut self, device: usize) {
        self.wake(device);
        let outputs = block_on(self.devices[device].tick()).expect("ticked");
        self.absorb(device, outputs);
    }

    /// Lets time pass until anything that can be decided has been.
    fn settle(&mut self) {
        self.run();
        for _ in 0..12 {
            for device in 0..self.devices.len() {
                self.tick(device);
            }
            self.run();
        }
    }

    fn write_to(&mut self, device: usize, group: GroupId, transaction: Transaction) {
        self.wake(device);
        let outputs = block_on(self.devices[device].write(group, transaction));
        self.absorb(device, outputs.expect("written"));
    }

    /// Writes to the group most tests are about.
    fn write(&mut self, device: usize, transaction: Transaction) {
        self.write_to(device, self.home, transaction);
    }

    fn reconfigure(
        &mut self,
        device: usize,
        group: GroupId,
        contributors: impl IntoIterator<Item = usize>,
    ) {
        let weights: Vec<_> = contributors
            .into_iter()
            .map(|it| (self.id(it), Role::contributor(1)))
            .collect();
        self.wake(device);
        let outputs = block_on(self.devices[device].reconfigure(group, weights));
        self.absorb(device, outputs.expect("asked"));
    }

    /// Asks for exactly these roles.
    fn assign(
        &mut self,
        device: usize,
        group: GroupId,
        roles: impl IntoIterator<Item = (usize, Role)>,
    ) {
        let roles: Vec<_> = roles
            .into_iter()
            .map(|(it, role)| (self.id(it), role))
            .collect();
        self.wake(device);
        let outputs = block_on(self.devices[device].reconfigure(group, roles));
        self.absorb(device, outputs.expect("asked"));
    }

    fn compact(&mut self, device: usize, group: GroupId) {
        self.wake(device);
        let outputs = block_on(self.devices[device].compact(group)).expect("compacted");
        self.absorb(device, outputs);
    }

    /// How many entries a device still keeps.
    fn entries_kept(&self, device: usize, group: GroupId) -> usize {
        let (start, end) = keys::range(keys::logs(group));
        let view = self.devices[device].store.view();

        block_on(super::step::scan(&view, &start, &end))
            .expect("scanned")
            .len()
    }

    /// Stops a device and starts it again from its store. Its
    /// connections are gone, and so is whatever was on its way to it.
    fn restart(&mut self, device: usize) {
        self.isolate(device);
        let store = self.devices[device].store.clone();
        self.wake(device);
        let reopened = block_on(Database::open(store, [0xEE; 32]));
        self.devices[device] = reopened.expect("reopened");
    }

    /// The device loses its store, and comes back under the same key
    /// with nothing but an invitation from the phone.
    fn forget(&mut self, device: usize, group: GroupId) {
        self.rejoin(device, group, PHONE);
    }

    /// The device loses its store, and is invited back by `inviter`.
    fn rejoin(&mut self, device: usize, group: GroupId, inviter: usize) {
        self.isolate(device);
        let invitation = self.devices[inviter].invite(group).expect("held");
        let reopened = block_on(Database::open(MemoryStore::default(), secret(device)));
        self.devices[device] = reopened.expect("opened");
        let (_, outputs) = block_on(self.devices[device].join(&invitation)).expect("joined");
        self.absorb(device, outputs);
    }

    fn reader(&self, device: usize) -> Reader<'_> {
        Reader(self, device)
    }

    fn read(&self, device: usize, query: &Query) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.wake(device);

        block_on(self.devices[device].read(query)).expect("read")
    }

    /// What `key` holds, as the device would show it right now.
    fn shown(&self, device: usize, group: GroupId, model: ModelId, key: &str) -> Option<String> {
        let found = self.read(device, &Query::key(group, model, key.as_bytes()));

        found.into_iter().next().map(|(_, value)| text(value))
    }

    /// What `key` holds for good.
    fn committed(
        &self,
        device: usize,
        group: GroupId,
        model: ModelId,
        key: &str,
    ) -> Option<String> {
        let query = Query::key(group, model, key.as_bytes()).committed();

        self.read(device, &query)
            .into_iter()
            .next()
            .map(|(_, value)| text(value))
    }
}

fn text(value: Vec<u8>) -> String {
    String::from_utf8(value).expect("tests write text")
}

fn notes(group: GroupId) -> ModelId {
    ModelId::kv(group, [1; 32])
}

fn names(group: GroupId) -> ModelId {
    ModelId::claims(group, [2; 32])
}

fn set(model: ModelId, key: &str, value: &str) -> Transaction {
    Transaction::new().set(model, key, value)
}

fn claim(model: ModelId, key: &str, value: &str) -> Transaction {
    Transaction::new().claim(model, key, value)
}

/// The phone, the laptop and the NAS contribute to one group. A friend
/// holds it too, and can only read.
fn household() -> Sim {
    Sim::group(4, 3).0
}

/// Usernames: only one device can have each.
fn usernames(cluster: &Sim) -> ModelId {
    ModelId::claims(cluster.home, [3; 32])
}

fn household_notes(cluster: &Sim) -> ModelId {
    ModelId::kv(cluster.home, [1; 32])
}

/// Claims a username, and writes a note saying whose it is, in one
/// transaction.
fn claim_name(cluster: &mut Sim, device: usize, name: &str, owner: &str) {
    let (names, notes) = (usernames(cluster), household_notes(cluster));
    let transaction =
        Transaction::new()
            .claim(names, name, owner)
            .set(notes, format!("owner/{name}"), owner);
    cluster.write(device, transaction);
}

/// Who holds a username, as far as the device knows.
fn holder(cluster: &Sim, device: usize, name: &str) -> Option<String> {
    let query = Query::key(cluster.home, usernames(cluster), name.as_bytes());

    run_query(cluster, device, &query)
}

fn write(cluster: &mut Sim, device: usize, key: &str, value: &str) {
    let model = household_notes(cluster);
    cluster.write(device, set(model, key, value));
}

/// The most recent value the device knows of.
fn read(cluster: &Sim, device: usize, key: &str) -> Option<String> {
    let query = Query::key(cluster.home, household_notes(cluster), key.as_bytes());

    run_query(cluster, device, &query)
}

/// Only what's committed on the device.
fn read_committed(cluster: &Sim, device: usize, key: &str) -> Option<String> {
    let query = Query::key(cluster.home, household_notes(cluster), key.as_bytes());

    run_query(cluster, device, &query.committed())
}

fn run_query(cluster: &Sim, device: usize, query: &Query) -> Option<String> {
    let (_, value) = cluster.read(device, query).pop()?;

    Some(String::from_utf8_lossy(&value).into_owned())
}

/// Every version in a device's storage that isn't committed, as stored.
fn stored_frontier(cluster: &Sim, device: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
    let (start, end) = keys::range(keys::uncommitted(cluster.home));
    let view = cluster.devices[device].store.view();

    block_on(super::step::scan(&view, &start, &end)).expect("scanned")
}
