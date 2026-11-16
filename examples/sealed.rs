//! A group only some of its holders can read.
//!
//! ```sh
//! cargo run --example sealed
//! ```
//!
//! A sealed group is encrypted to the devices given a key to it. Its
//! contributors read and write. A reader reads and never writes. A
//! witness counts toward the quorum and carries everything, and can
//! read none of it: here it's the only path between the others. Read
//! access is taken back by taking the role back.

#![expect(clippy::print_stdout, reason = "an example says what it did")]

mod support;

use support::{Network, block_on};
use woodland::{Error, GroupId, ModelId, Role, Transaction};

const PHONE: usize = 0;
const LAPTOP: usize = 1;
const NAS: usize = 2;
const FRIEND: usize = 3;

/// The phone asks for these roles. The friend reads, or has no role.
fn assign(network: &mut Network, group: GroupId, friend_reads: bool) -> Result<(), Error> {
    // A sealing key is a device's public key for being sent secrets.
    // It's what a role that reads is given to.
    let id = |device: usize| network.devices[device].database.id();
    let key = |device: usize| network.devices[device].database.sealing_key();
    let mut roles = vec![
        (id(PHONE), Role::contributor(1).sealed_to(key(PHONE))),
        (id(LAPTOP), Role::contributor(1).sealed_to(key(LAPTOP))),
        (id(NAS), Role::witness(1)),
    ];
    if friend_reads {
        roles.push((id(FRIEND), Role::reader(key(FRIEND))));
    }
    let outputs = block_on(network.devices[PHONE].database.reconfigure(group, roles))?;
    network.absorb(PHONE, outputs);
    network.settle();

    Ok(())
}

fn report(network: &Network, group: GroupId, key: &str) {
    let diary = ModelId::kv(group, [1; 32]);
    for (device, held) in network.devices.iter().enumerate() {
        let reads = held.database.reads(group);
        let look = network.look(device, group, diary, key);
        println!("{look} (can read: {reads})");
    }
}

fn main() -> Result<(), Error> {
    let mut network = Network::default();
    for name in ["phone", "laptop", "nas", "friend"] {
        network.add(name);
    }
    let (group, outputs) = block_on(network.devices[PHONE].database.found_sealed([7; 32]))?;
    network.absorb(PHONE, outputs);
    let diary = ModelId::kv(group, [1; 32]);
    let invitation = network.devices[PHONE].database.invite(group);
    let invitation = invitation.expect("the phone holds the group");
    for device in [LAPTOP, NAS, FRIEND] {
        let (_, outputs) = block_on(network.devices[device].database.join(&invitation))?;
        network.absorb(device, outputs);
        // Everything goes by way of the nas.
        network.connect(NAS, device);
    }
    network.connect(NAS, PHONE);

    println!("-- the phone and laptop write, the nas witnesses, the friend reads");
    assign(&mut network, group, true)?;
    let write = Transaction::new().set(diary, "monday", "the party is on saturday");
    let outputs = block_on(network.devices[PHONE].database.write(group, write))?;
    network.absorb(PHONE, outputs);
    network.settle();
    // The nas voted for the checkpoint that committed it, and passed
    // it to everyone else. It holds it, and can't say what it says.
    report(&network, group, "monday");

    println!("-- the friend's role is taken back");
    assign(&mut network, group, false)?;
    let write = Transaction::new().set(diary, "tuesday", "the friend mustn't know");
    let outputs = block_on(network.devices[LAPTOP].database.write(group, write))?;
    network.absorb(LAPTOP, outputs);
    network.settle();
    // What's written from here on is under a secret the friend was
    // never sent. What it read before, it read.
    report(&network, group, "tuesday");

    println!("-- the nas is unplugged: the phone and laptop meet directly");
    network.disconnect(NAS, PHONE);
    network.disconnect(NAS, LAPTOP);
    network.connect(PHONE, LAPTOP);
    let write = Transaction::new().set(diary, "wednesday", "cake ordered");
    let outputs = block_on(network.devices[PHONE].database.write(group, write))?;
    network.absorb(PHONE, outputs);
    network.settle();
    // Two of three votes is still a quorum.
    report(&network, group, "wednesday");

    Ok(())
}
