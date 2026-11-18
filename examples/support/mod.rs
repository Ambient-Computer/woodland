//! What every example needs and the library leaves to the application:
//! somewhere to run a step, and a way for messages to get from one
//! device to another.
//!
//! The library is a state machine. Each call to a [`Database`] is one
//! step, and returns [`Outputs`]: messages to deliver, what may read
//! differently, and whether there's more to do. An application gives
//! the messages to its transport. Here the transport is a queue in
//! memory, and "connected" means the queue will carry a message
//! between two devices.

use std::collections::{BTreeSet, VecDeque};

mod executor;

pub(crate) use executor::block_on;
use woodland::{Database, GroupId, MemoryStore, ModelId, Outcome, Outputs, Query};

/// A device: its database, and what the application keeps beside it.
#[derive(Debug)]
pub(crate) struct Device {
    pub(crate) name: &'static str,
    pub(crate) database: Database<MemoryStore>,
    /// Whether its last step left work that waits for nothing.
    unfinished: bool,
    /// What it has been told became of the transactions it wrote.
    pub(crate) told: Vec<Outcome>,
}

/// A few devices, and which of them can reach each other right now.
#[derive(Debug, Default)]
pub(crate) struct Network {
    pub(crate) devices: Vec<Device>,
    links: BTreeSet<(usize, usize)>,
    /// Messages on their way: from, to, and what.
    queue: VecDeque<(usize, usize, Vec<u8>)>,
}

impl Network {
    /// A new device with an empty store. Returns its place in
    /// [`Network::devices`].
    pub(crate) fn add(&mut self, name: &'static str) -> usize {
        // The secret is the device's signing key, made once and kept
        // in its store. A real one is random.
        let secret = [u8::try_from(self.devices.len() + 1).expect("a few devices"); 32];
        #[cfg_attr(not(feature = "blobs"), expect(unused_mut, reason = "set with blobs"))]
        let mut database = block_on(Database::open(MemoryStore::new(), secret)).expect("opened");
        // With blobs, a device says which it fetches before it holds
        // a group. There's no default.
        #[cfg(feature = "blobs")]
        drop(block_on(database.set_blob_policy(woodland::BlobPolicy::None)).expect("said"));
        self.devices.push(Device {
            name,
            database,
            unfinished: false,
            told: Vec::new(),
        });

        self.devices.len() - 1
    }

    /// Does what a step asked for: sends its messages to whoever can
    /// be reached, and remembers the rest.
    pub(crate) fn absorb(&mut self, device: usize, outputs: Outputs) {
        let told = outputs.outcomes.into_iter().map(|(_, outcome)| outcome);
        self.devices[device].told.extend(told);
        self.devices[device].unfinished = outputs.unfinished;
        for (to, message) in outputs.sends {
            let to = self.devices.iter().position(|it| it.database.id() == to);
            let to = to.expect("a message for a device in the example");
            if self.links.contains(&(device.min(to), device.max(to))) {
                self.queue.push_back((device, to, message));
            }
        }
    }

    /// The two can reach each other. Each is told, and says what it
    /// holds.
    pub(crate) fn connect(&mut self, a: usize, b: usize) {
        self.links.insert((a.min(b), a.max(b)));
        for (here, there) in [(a, b), (b, a)] {
            let there = self.devices[there].database.id();
            let outputs = block_on(self.devices[here].database.connected(there));
            self.absorb(here, outputs.expect("connected"));
        }
    }

    /// The two can't reach each other, and whatever was on its way
    /// between them is lost.
    pub(crate) fn disconnect(&mut self, a: usize, b: usize) {
        self.links.remove(&(a.min(b), a.max(b)));
        let (id_a, id_b) = (self.devices[a].database.id(), self.devices[b].database.id());
        self.devices[a].database.disconnected(id_b);
        self.devices[b].database.disconnected(id_a);
        self.queue
            .retain(|&(from, to, _)| (from.min(to), from.max(to)) != (a.min(b), a.max(b)));
    }

    /// Delivers every message, and every message that follows, and
    /// has every device finish what it started.
    pub(crate) fn run(&mut self) {
        loop {
            if let Some((from, to, message)) = self.queue.pop_front() {
                let from = self.devices[from].database.id();
                let outputs = block_on(self.devices[to].database.receive(from, &message));
                self.absorb(to, outputs.expect("received"));
            } else if let Some(device) = self.devices.iter().position(|it| it.unfinished) {
                let outputs = block_on(self.devices[device].database.resume());
                self.absorb(device, outputs.expect("resumed"));
            } else {
                break;
            }
        }
    }

    /// Lets time pass until nothing more comes of it. A device
    /// attempts a checkpoint when it's told time has passed, which an
    /// application does from a timer.
    pub(crate) fn settle(&mut self) {
        self.run();
        for _ in 0..12 {
            for device in 0..self.devices.len() {
                let outputs = block_on(self.devices[device].database.tick());
                self.absorb(device, outputs.expect("ticked"));
            }
            self.run();
        }
    }

    /// What `key` holds on `device`: as it shows right now, and as
    /// it's committed.
    pub(crate) fn look(&self, device: usize, group: GroupId, model: ModelId, key: &str) -> String {
        let read = |query: Query| {
            let answer = block_on(self.devices[device].database.read(&query)).expect("read");
            let found = answer.into_found().into_iter().next();

            found.map_or("nothing".into(), |(_, value)| {
                format!("{:?}", String::from_utf8_lossy(&value))
            })
        };
        let query = Query::key(group, model, key.as_bytes());
        let (shown, committed) = (read(query.clone()), read(query.committed()));

        format!(
            "{:>7} | {key}: shows {shown}, committed {committed}",
            self.devices[device].name
        )
    }
}
