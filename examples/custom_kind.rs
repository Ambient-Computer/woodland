//! A kind of model the application defines.
//!
//! ```sh
//! cargo run --example custom_kind
//! ```
//!
//! The library's kinds are built from one contract: an operation says
//! what it requires and what it writes. An application can add a kind
//! by saying the same for its own operations. This one is a tally,
//! where `add key n` adds to a count, and two devices' additions both
//! count instead of one replacing the other.

#![expect(clippy::print_stdout, reason = "an example says what it did")]

mod support;

use support::{Network, block_on};
use woodland::{Declared, Error, Extension, ModelId, Role, Transaction, extend};

const PHONE: usize = 0;
const LAPTOP: usize = 1;

/// An application numbers its own kinds, from 0 to 63.
const TALLIES: u8 = 0;

/// What an operation does. `None` if it isn't one: then the
/// transaction it's in doesn't happen, on any device.
fn declare(operation: &[u8]) -> Option<Declared> {
    let text = std::str::from_utf8(operation).ok()?;
    let ["add", key, count] = text.split(' ').collect::<Vec<_>>()[..] else {
        return None;
    };
    count.parse::<u64>().ok()?;

    Some(Declared {
        // It depends on nothing, so nothing can stop it.
        requires: Vec::new(),
        writes: vec![(key.as_bytes().to_vec(), Some(count.as_bytes().to_vec()))],
    })
}

/// What a key holds once a write is combined with what it held. It's
/// called in the order every device agrees on, so it needn't commute.
fn combine(held: Option<&[u8]>, write: &[u8]) -> Vec<u8> {
    let number = |bytes: &[u8]| std::str::from_utf8(bytes).ok()?.parse::<u64>().ok();
    let held = held.and_then(number).unwrap_or(0);

    held.saturating_add(number(write).unwrap_or(0))
        .to_string()
        .into_bytes()
}

fn main() -> Result<(), Error> {
    // Kinds are part of the application, like its code: registered
    // once for the process, before a database is opened.
    let added = extend(Extension {
        number: TALLIES,
        declare,
        combine: Some(combine),
    });
    assert!(added);

    let mut network = Network::default();
    network.add("phone");
    network.add("laptop");
    let (group, outputs) = block_on(network.devices[PHONE].database.found([7; 32]))?;
    network.absorb(PHONE, outputs);
    let invitation = network.devices[PHONE].database.invite(group);
    let invitation = invitation.expect("the phone holds the group");
    let (_, outputs) = block_on(network.devices[LAPTOP].database.join(&invitation))?;
    network.absorb(LAPTOP, outputs);
    network.connect(PHONE, LAPTOP);
    let ids = network.devices.iter().map(|it| it.database.id());
    let roles: Vec<_> = ids.map(|id| (id, Role::contributor(1))).collect();
    let outputs = block_on(network.devices[PHONE].database.reconfigure(group, roles))?;
    network.absorb(PHONE, outputs);
    network.settle();

    let tallies = ModelId::of_kind(TALLIES, group, [1; 32]).expect("a number below 64");
    let add = |network: &mut Network, device: usize, count: u64| {
        let write = Transaction::new().change(tallies, format!("add visits {count}"));
        let outputs = block_on(network.devices[device].database.write(group, write));
        network.absorb(device, outputs.expect("a contributor writes"));
    };

    println!("-- one visit, committed");
    add(&mut network, PHONE, 1);
    network.settle();
    println!("{}", network.look(PHONE, group, tallies, "visits"));

    println!("-- offline: the phone counts 6 more, the laptop 100");
    network.disconnect(PHONE, LAPTOP);
    add(&mut network, PHONE, 2);
    add(&mut network, PHONE, 4);
    add(&mut network, LAPTOP, 100);
    for device in [PHONE, LAPTOP] {
        println!("{}", network.look(device, group, tallies, "visits"));
    }

    println!("-- they meet");
    network.connect(PHONE, LAPTOP);
    network.settle();
    for device in [PHONE, LAPTOP] {
        println!("{}", network.look(device, group, tallies, "visits"));
    }

    Ok(())
}
