//! A device is removed, and later let back in.
//!
//! ```sh
//! cargo run --example removal
//! ```
//!
//! Three devices contribute. The laptop goes missing with something
//! written that nobody heard. The other two remove it, which needs
//! neither its consent nor its presence. What it wrote before the cut
//! never counts, whenever it turns up. When it's found and given its
//! role back, it's the same device, and what it writes from then on
//! counts like anyone's.

#![expect(clippy::print_stdout, reason = "an example says what it did")]

mod support;

use support::{Network, block_on};
use woodland::{Error, GroupId, ModelId, Outcome, Role, Transaction};

const PHONE: usize = 0;
const LAPTOP: usize = 1;
const NAS: usize = 2;

/// The phone asks for exactly these contributors, with equal say. A
/// quorum of those there are now has to agree.
fn contributors(network: &mut Network, group: GroupId, devices: &[usize]) -> Result<(), Error> {
    let id = |&device: &usize| network.devices[device].database.id();
    let roles: Vec<_> = devices.iter().map(id).collect();
    let roles = roles.into_iter().map(|id| (id, Role::contributor(1)));
    let outputs = block_on(network.devices[PHONE].database.reconfigure(group, roles))?;
    network.absorb(PHONE, outputs);
    network.settle();

    Ok(())
}

fn write(network: &mut Network, device: usize, group: GroupId, note: (&str, &str)) {
    let notes = ModelId::kv(group, [1; 32]);
    let write = Transaction::new().set(notes, note.0, note.1);
    let outputs = block_on(network.devices[device].database.write(group, write));
    network.absorb(device, outputs.expect("a contributor writes"));
}

fn main() -> Result<(), Error> {
    let mut network = Network::default();
    for name in ["phone", "laptop", "nas"] {
        network.add(name);
    }
    let (group, outputs) = block_on(network.devices[PHONE].database.found([7; 32]))?;
    network.absorb(PHONE, outputs);
    let notes = ModelId::kv(group, [1; 32]);
    let invitation = network.devices[PHONE].database.invite(group);
    let invitation = invitation.expect("the phone holds the group");
    for device in [LAPTOP, NAS] {
        let (_, outputs) = block_on(network.devices[device].database.join(&invitation))?;
        network.absorb(device, outputs);
        network.connect(PHONE, device);
    }
    network.connect(LAPTOP, NAS);
    contributors(&mut network, group, &[PHONE, LAPTOP, NAS])?;

    println!("-- the laptop is left in a taxi, with a note nobody heard");
    network.disconnect(LAPTOP, PHONE);
    network.disconnect(LAPTOP, NAS);
    write(
        &mut network,
        LAPTOP,
        group,
        ("draft", "written in the taxi"),
    );
    println!("{}", network.look(LAPTOP, group, notes, "draft"));

    println!("-- the phone and the nas remove it: two of three is a quorum");
    contributors(&mut network, group, &[PHONE, NAS])?;
    write(&mut network, PHONE, group, ("todo", "buy a laptop"));
    network.settle();
    println!("{}", network.look(NAS, group, notes, "todo"));

    println!("-- the taxi driver brings it back, and it reconnects");
    network.connect(LAPTOP, PHONE);
    network.connect(LAPTOP, NAS);
    network.settle();
    // It learns it was removed. What it wrote under the checkpoint
    // before that one is void, here and anywhere it was carried to.
    for device in [LAPTOP, PHONE] {
        println!("{}", network.look(device, group, notes, "draft"));
    }
    assert!(network.devices[LAPTOP].told.contains(&Outcome::Discarded));
    println!(" laptop | was told: {:?}", network.devices[LAPTOP].told);
    // It still holds the group and reads it. It can't write.
    println!("{}", network.look(LAPTOP, group, notes, "todo"));
    let write_again = Transaction::new().set(notes, "draft", "again");
    let refused = block_on(network.devices[LAPTOP].database.write(group, write_again));
    println!(" laptop | writes while removed: {refused:?}");

    println!("-- it's given its role back: the same key, the same device");
    contributors(&mut network, group, &[PHONE, LAPTOP, NAS])?;
    write(&mut network, LAPTOP, group, ("draft", "written at home"));
    network.settle();
    for device in [LAPTOP, PHONE, NAS] {
        println!("{}", network.look(device, group, notes, "draft"));
    }

    Ok(())
}
