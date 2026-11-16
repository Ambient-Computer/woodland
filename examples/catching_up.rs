//! Small devices, a device that keeps everything, and catching up.
//!
//! ```sh
//! cargo run --example catching_up
//! ```
//!
//! What's committed can be kept as its result and not as the history
//! that led to it. A phone sheds history to stay small. A nas keeps
//! it all. A tablet linked late, and a laptop back from a long time
//! away, are brought up to date with committed state, checked against
//! a checkpoint, and not by replaying every transaction. History is
//! still there for whoever wants it back.

#![expect(clippy::print_stdout, reason = "an example says what it did")]

mod support;

use support::{Network, block_on};
use woodland::{ClockId, Error, GroupId, ModelId, Query, Role, Transaction};

const PHONE: usize = 0;
const NAS: usize = 1;
const LAPTOP: usize = 2;
const TABLET: usize = 3;

/// How many notes the device has committed, and whether it holds
/// every transaction that was ever committed.
fn report(network: &Network, group: GroupId) {
    let notes = ModelId::kv(group, [1; 32]);
    for device in &network.devices {
        let query = Query::prefix(group, notes, b"").committed();
        let held = block_on(device.database.read(&query)).map(|it| it.found().len());
        // Everything in a group that isn't given a clock of its own
        // is in the group's own.
        let history = device.database.is_archive(group, ClockId::from(group));
        println!(
            "{:>7} | {:>2} notes committed, whole history: {history}",
            device.name,
            held.unwrap_or(0)
        );
    }
}

fn write(network: &mut Network, group: GroupId, from: usize, count: usize) -> Result<(), Error> {
    let notes = ModelId::kv(group, [1; 32]);
    for note in from..from + count {
        let write = Transaction::new().set(notes, format!("note {note:02}"), "x");
        let outputs = block_on(network.devices[PHONE].database.write(group, write))?;
        network.absorb(PHONE, outputs);
        network.settle();
    }

    Ok(())
}

fn main() -> Result<(), Error> {
    let mut network = Network::default();
    for name in ["phone", "nas", "laptop", "tablet"] {
        network.add(name);
    }
    let (group, outputs) = block_on(network.devices[PHONE].database.found([7; 32]))?;
    network.absorb(PHONE, outputs);
    let invitation = network.devices[PHONE].database.invite(group);
    let invitation = invitation.expect("the phone holds the group");
    for device in [NAS, LAPTOP] {
        let (_, outputs) = block_on(network.devices[device].database.join(&invitation))?;
        network.absorb(device, outputs);
        network.connect(PHONE, device);
    }
    // The phone and the nas decide. The laptop only follows: it holds
    // the group and reads it, and nothing waits for it.
    let (phone, nas) = (
        network.devices[PHONE].database.id(),
        network.devices[NAS].database.id(),
    );
    let roles = [(phone, Role::contributor(1)), (nas, Role::contributor(1))];
    let outputs = block_on(network.devices[PHONE].database.reconfigure(group, roles))?;
    network.absorb(PHONE, outputs);
    network.settle();

    println!("-- the laptop is put in a drawer, and a lot is written");
    network.disconnect(PHONE, LAPTOP);
    write(&mut network, group, 0, 20)?;
    report(&network, group);

    println!("-- the phone sheds the history of what's committed");
    let outputs = block_on(network.devices[PHONE].database.compact(group))?;
    network.absorb(PHONE, outputs);
    network.settle();
    report(&network, group);

    println!("-- a tablet is linked by the phone, which has no history to give");
    let invitation = network.devices[PHONE].database.invite(group);
    let invitation = invitation.expect("the phone holds the group");
    let (_, outputs) = block_on(network.devices[TABLET].database.join(&invitation))?;
    network.absorb(TABLET, outputs);
    network.connect(PHONE, TABLET);
    network.settle();
    report(&network, group);

    println!("-- the laptop comes out of the drawer, and asks the nas for state");
    // The nas could send it every transaction it missed. It asks to be
    // sent what they came to instead. The result is the same.
    let outputs = block_on(network.devices[LAPTOP].database.skip_ahead(group))?;
    network.absorb(LAPTOP, outputs);
    network.connect(NAS, LAPTOP);
    network.settle();
    report(&network, group);

    println!("-- it carries on with transactions from there");
    write(&mut network, group, 20, 2)?;
    report(&network, group);

    println!("-- the tablet wants the history after all, and the nas has it");
    network.connect(NAS, TABLET);
    let restoring = network.devices[TABLET]
        .database
        .restore(group, group.into());
    let outputs = block_on(restoring)?;
    network.absorb(TABLET, outputs);
    network.settle();
    report(&network, group);
    let notes = ModelId::kv(group, [1; 32]);
    println!("{}", network.look(TABLET, group, notes, "note 21"));

    Ok(())
}
