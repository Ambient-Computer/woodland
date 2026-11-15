//! A simulated network of devices, each with its own store.
//!
//! The simulation plays the application: it delivers what each step
//! asks to send, in an order the test controls, and nothing else moves.

mod checkpoints;
mod membership;
mod random;
mod single;
mod sync;

use std::collections::{BTreeSet, VecDeque};

use ed25519_dalek::VerifyingKey;

use crate::agreement::AgreementId;
use crate::io::Changed;
use crate::model::{ModelId, Query};
use crate::store::MemoryStore;
use crate::testing::block_on;

use super::{Database, Error, Outputs, Transaction};

const PHONE: usize = 0;
const LAPTOP: usize = 1;
const NAS: usize = 2;

struct Sim {
    devices: Vec<Database<MemoryStore>>,
    /// Messages on their way: from, to, bytes.
    queue: VecDeque<(usize, usize, Vec<u8>)>,
    links: BTreeSet<(usize, usize)>,
    changes: Vec<Vec<Changed>>,
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
        }
    }

    /// A group founded by the phone and held by everyone, with the first
    /// `contributors` devices as equal contributors, all connected.
    fn group(devices: usize, contributors: usize) -> (Self, AgreementId) {
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

        (sim, group)
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
        let outputs = block_on(self.devices[to].receive(from, &message)).expect("received");
        self.absorb(to, outputs);

        true
    }

    /// Delivers everything, and everything that follows from it.
    fn run(&mut self) {
        let mut delivered = 0;
        while self.deliver() {
            delivered += 1;
            assert!(delivered < 100_000, "the devices never stop talking");
        }
    }

    fn tick(&mut self, device: usize) {
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

    fn write(&mut self, device: usize, group: AgreementId, transaction: Transaction) {
        let outputs = block_on(self.devices[device].write(group, transaction));
        self.absorb(device, outputs.expect("written"));
    }

    fn reconfigure(
        &mut self,
        device: usize,
        group: AgreementId,
        contributors: impl IntoIterator<Item = usize>,
    ) {
        let weights: Vec<_> = contributors
            .into_iter()
            .map(|it| (self.id(it), 1))
            .collect();
        let outputs = block_on(self.devices[device].reconfigure(group, weights));
        self.absorb(device, outputs.expect("asked"));
    }

    /// Stops a device and starts it again from its store. Its
    /// connections are gone, and so is whatever was on its way to it.
    fn restart(&mut self, device: usize) {
        self.isolate(device);
        let store = self.devices[device].store.clone();
        let reopened = block_on(Database::open(store, [0xEE; 32]));
        self.devices[device] = reopened.expect("reopened");
    }

    fn read(&self, device: usize, query: &Query) -> Vec<(Vec<u8>, Vec<u8>)> {
        block_on(self.devices[device].read(query)).expect("read")
    }

    /// What `key` holds, as the device would show it right now.
    fn shown(
        &self,
        device: usize,
        group: AgreementId,
        model: ModelId,
        key: &str,
    ) -> Option<String> {
        let found = self.read(device, &Query::key(group, model, key.as_bytes()));

        found.into_iter().next().map(|(_, value)| text(value))
    }

    /// What `key` holds for good.
    fn committed(
        &self,
        device: usize,
        group: AgreementId,
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

fn notes(group: AgreementId) -> ModelId {
    ModelId::kv(group, [1; 32])
}

fn names(group: AgreementId) -> ModelId {
    ModelId::claims(group, [2; 32])
}

fn set(model: ModelId, key: &str, value: &str) -> Transaction {
    Transaction::new().set(model, key, value)
}

fn claim(model: ModelId, key: &str, value: &str) -> Transaction {
    Transaction::new().claim(model, key, value)
}
