//! Two devices that are rarely online together.
//!
//! ```sh
//! cargo run --example devices
//! ```
//!
//! A phone founds a group and links a laptop. Both write while they
//! can't reach each other, including to the same key and to the same
//! claim. When they meet they agree, and a checkpoint makes it final.

#![expect(clippy::print_stdout, reason = "an example says what it did")]

mod support;

use support::{Network, block_on};
use woodland::{Error, ModelId, Outcome, Role, Transaction};

const PHONE: usize = 0;
const LAPTOP: usize = 1;

fn main() -> Result<(), Error> {
    let mut network = Network::default();
    network.add("phone");
    network.add("laptop");

    let (group, outputs) = block_on(network.devices[PHONE].database.found([7; 32]))?;
    network.absorb(PHONE, outputs);
    // Notes are a map where the last write wins. Names are a map of
    // claims: a key belongs to whoever took it first.
    let notes = ModelId::kv(group, [1; 32]);
    let names = ModelId::claims(group, [2; 32]);

    println!("-- the laptop is linked");
    // An invitation says where the group began and how far it has
    // got. However it travels (a QR code, a link), the laptop trusts
    // whoever gave it.
    let invitation = network.devices[PHONE].database.invite(group);
    let invitation = invitation.expect("the phone holds the group");
    let (_, outputs) = block_on(network.devices[LAPTOP].database.join(&invitation))?;
    network.absorb(LAPTOP, outputs);
    network.connect(PHONE, LAPTOP);
    network.run();

    // Holding a group isn't writing to it. A contributor is admitted
    // by the contributors there are, in a checkpoint.
    let write = Transaction::new().set(notes, "todo", "too early");
    let early = block_on(network.devices[LAPTOP].database.write(group, write));
    println!(" laptop | writes before it's a contributor: {early:?}");

    let (phone, laptop) = (
        network.devices[PHONE].database.id(),
        network.devices[LAPTOP].database.id(),
    );
    let roles = [
        (phone, Role::contributor(1)),
        (laptop, Role::contributor(1)),
    ];
    let outputs = block_on(network.devices[PHONE].database.reconfigure(group, roles))?;
    network.absorb(PHONE, outputs);
    network.settle();

    println!("-- they go their separate ways, and both write");
    network.disconnect(PHONE, LAPTOP);
    let mut write = |device: usize, transaction: Transaction| -> Result<(), Error> {
        let outputs = block_on(network.devices[device].database.write(group, transaction))?;
        network.absorb(device, outputs);

        Ok(())
    };
    write(PHONE, Transaction::new().set(notes, "todo", "milk"))?;
    write(LAPTOP, Transaction::new().set(notes, "todo", "eggs"))?;
    write(PHONE, Transaction::new().claim(names, "ada", "the phone's"))?;
    write(
        LAPTOP,
        Transaction::new().claim(names, "ada", "the laptop's"),
    )?;
    // Each shows its own. Neither is committed: with two equal
    // contributors, a quorum is both of them.
    for device in [PHONE, LAPTOP] {
        println!("{}", network.look(device, group, notes, "todo"));
        println!("{}", network.look(device, group, names, "ada"));
    }

    println!("-- they meet");
    network.connect(PHONE, LAPTOP);
    network.run();
    // No checkpoint yet, and they already agree: both hold the same
    // transactions and order them the same way.
    for device in [PHONE, LAPTOP] {
        println!("{}", network.look(device, group, notes, "todo"));
        println!("{}", network.look(device, group, names, "ada"));
    }

    println!("-- time passes, and a checkpoint is agreed");
    network.settle();
    for device in [PHONE, LAPTOP] {
        println!("{}", network.look(device, group, notes, "todo"));
        println!("{}", network.look(device, group, names, "ada"));
    }

    // One claim took the name. The other device is told its
    // transaction was committed and didn't take effect.
    for device in &network.devices {
        println!("{:>7} | was told: {:?}", device.name, device.told);
    }
    let rejected = |device: usize| network.devices[device].told.contains(&Outcome::Rejected);
    assert_ne!(rejected(PHONE), rejected(LAPTOP));

    Ok(())
}
