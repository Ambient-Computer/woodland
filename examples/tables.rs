//! Rows and columns, and a change to rows its author never saw.
//!
//! ```sh
//! cargo run --example tables
//! ```
//!
//! A table's schema and rows are in the same history, so a row is
//! never read against the wrong columns. A change that picks its rows
//! by a filter is decided when it's committed, over every row
//! committed by then, including one written elsewhere at the same
//! time. An index is a device's own business.

#![expect(clippy::print_stdout, reason = "an example says what it did")]

mod support;

use support::{Network, block_on};
use woodland::{ColumnId, Error, Filter, GroupId, ModelId, Query, Role, Table, Transaction, Value};

const PHONE: usize = 0;
const LAPTOP: usize = 1;

fn report(network: &Network, group: GroupId, people: ModelId, team: ColumnId) {
    for device in &network.devices {
        let read = |query: Query| block_on(device.database.read(&query)).expect("read");
        let rows = |query: Query| {
            let table = Table::read(read(query).found());
            let rows = table.iter().flat_map(Table::rows);
            let row = |row: &woodland::Row| {
                let key = String::from_utf8_lossy(row.key()).into_owned();

                format!("{key}: {:?}", row.get(team))
            };

            rows.map(row).collect::<Vec<_>>().join(", ")
        };
        let query = Query::table(group, people);
        println!("{:>7} | shows     [{}]", device.name, rows(query.clone()));
        println!(
            "{:>7} | committed [{}]",
            device.name,
            rows(query.committed())
        );
    }
}

fn main() -> Result<(), Error> {
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
    let write = |network: &mut Network, device: usize, transaction: Transaction| {
        let outputs = block_on(network.devices[device].database.write(group, transaction));
        network.absorb(device, outputs.expect("a contributor writes"));
    };

    // Tables and columns are named by IDs made where they're created,
    // so two devices that each make one offline make two.
    let people = ModelId::table(group, [1; 32]);
    let (name, team) = (
        ColumnId::text(people, [1; 32]),
        ColumnId::text(people, [2; 32]),
    );
    let person = |who: &str, side: &str| {
        let cells = [(name, Value::from(who)), (team, Value::from(side))];

        Transaction::new().insert(people, who, cells)
    };

    println!("-- a table, and two people in it");
    // A transaction is all or nothing, across models. This one takes
    // the name "people" and creates the table. Had another device
    // taken the name first, neither would happen.
    let names = ModelId::claims(group, [9; 32]);
    let columns = [(name, "name"), (team, "team")];
    let create = Transaction::new().claim(names, "people", "the phone's table");
    write(&mut network, PHONE, create.create_table(people, columns));
    write(&mut network, PHONE, person("ada", "red"));
    write(&mut network, PHONE, person("bob", "green"));
    network.settle();
    println!("{}", network.look(LAPTOP, group, names, "people"));
    report(&network, group, people, team);

    println!("-- offline: the phone renames red to blue, the laptop adds to red");
    network.disconnect(PHONE, LAPTOP);
    let red = Filter::all().is(team, "red");
    let rename = Transaction::new().update_where(people, red, [(team, Value::from("blue"))]);
    write(&mut network, PHONE, rename);
    write(&mut network, LAPTOP, person("cy", "red"));
    report(&network, group, people, team);
    // The rename shows nowhere yet. It's a change that waits to be
    // committed, and the application can say so.
    let pending = block_on(network.devices[PHONE].database.pending(group))?;
    println!("  phone | {} change waiting to be committed", pending.len());

    println!("-- they meet, and it's committed");
    network.connect(PHONE, LAPTOP);
    network.settle();
    // The rename is applied to every red row there is when it's
    // committed. Whether cy is one depends on which came first in
    // the order both devices agree on, and they do agree.
    report(&network, group, people, team);

    println!("-- the laptop indexes the team column, for itself");
    let outputs = block_on(
        network.devices[LAPTOP]
            .database
            .add_index(group, people, team),
    )?;
    network.absorb(LAPTOP, outputs);
    network.run();
    assert!(
        network.devices[LAPTOP]
            .database
            .has_index(group, people, team)
    );
    let blue = Query::lookup(group, people, team, &Value::from("blue"));
    let found = block_on(network.devices[LAPTOP].database.read(&blue))?;
    let keys = Table::row_keys(found.found());
    let keys: Vec<_> = keys
        .iter()
        .map(|key| String::from_utf8_lossy(key))
        .collect();
    println!(" laptop | on the blue team: {keys:?}");
    assert!(
        !network.devices[PHONE]
            .database
            .has_index(group, people, team)
    );

    Ok(())
}
