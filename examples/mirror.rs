//! A copy of a table, kept outside the library and kept current.
//!
//! ```sh
//! cargo run --example mirror
//! ```
//!
//! The library reads ranges and says what changed. It doesn't join,
//! count, or search. Anything that does is built beside it, from two
//! things: everything that's committed, read a part at a time, and
//! what each step says it committed after that. A search index, an
//! export, and a database someone else queries are all this.
//!
//! Here it's a count of people by team.

#![expect(clippy::print_stdout, reason = "an example says what it did")]

#[path = "support/executor.rs"]
mod executor;

use std::collections::BTreeMap;

use executor::block_on;
use woodland::{
    Changed, ColumnId, Database, Error, GroupId, MemoryStore, ModelId, Query, Table, Transaction,
    Value,
};

/// What's kept beside the library: who's on which team, as of a
/// checkpoint.
#[derive(Debug, Default)]
struct Mirror {
    teams: BTreeMap<Vec<u8>, String>,
    /// The checkpoint this is exactly the state of. Kept with the copy,
    /// it says after a restart whether anything was missed.
    at: Option<u64>,
}

impl Mirror {
    fn count(&self) -> BTreeMap<&str, usize> {
        let mut count = BTreeMap::new();
        for team in self.teams.values() {
            *count.entry(team.as_str()).or_default() += 1;
        }

        count
    }
}

struct App {
    phone: Database<MemoryStore>,
    group: GroupId,
    people: ModelId,
    team: ColumnId,
    mirror: Mirror,
}

impl App {
    /// Writes, lets time pass so that it's committed, and has the
    /// mirror take in what each step reports.
    fn write(&mut self, transaction: Transaction) -> Result<(), Error> {
        let mut changed = block_on(self.phone.write(self.group, transaction))?.changed;
        let mut outputs = block_on(self.phone.tick())?;
        loop {
            changed.extend(outputs.changed);
            if !outputs.unfinished {
                break;
            }
            outputs = block_on(self.phone.resume())?;
        }

        for change in changed {
            self.follow(&change)?;
        }

        Ok(())
    }

    fn follow(&mut self, change: &Changed) -> Result<(), Error> {
        match change {
            // Exactly a checkpoint's state: the moment to save the
            // copy, or to publish it.
            Changed::Checkpoint { height, .. } => {
                self.mirror.at = Some(*height);
                println!("at checkpoint {height}: {:?}", self.mirror.count());
            }
            // A report says where to look. The row is read again.
            Changed::Key {
                committed: true, ..
            } => {
                let Some(key) = change.row() else {
                    return Ok(());
                };

                let query = Query::row(self.group, self.people, &key).committed();
                let answer = block_on(self.phone.read(&query))?;
                let table = Table::read(answer.found());
                match table.as_ref().and_then(|table| table.row(&key)) {
                    Some(row) => self.mirror.teams.insert(key, text(row.get(self.team))),
                    None => self.mirror.teams.remove(&key),
                };
            }
            // Everything may be different. A mirror starts over.
            Changed::Everything { .. } => self.begin()?,
            _ => {}
        }

        Ok(())
    }

    /// Reads everything that's committed, two rows at a time. A real
    /// one would read hundreds, and could let steps run in between:
    /// whatever they commit, they report.
    fn begin(&mut self) -> Result<(), Error> {
        self.mirror = Mirror::default();
        let mut next = Some(Query::table(self.group, self.people).committed());
        while let Some(query) = next {
            let part = block_on(self.phone.read_some(&query, 2))?;
            let reached = part.answer.reached().expect("a committed read");
            if reached.settled {
                self.mirror.at = Some(reached.checkpoint);
            }

            let table = Table::read(part.answer.found());
            let rows = table.iter().flat_map(Table::rows);
            println!("read {} rows", rows.clone().count());
            for row in rows {
                let team = text(row.get(self.team));
                self.mirror.teams.insert(row.key().to_vec(), team);
            }

            next = part.rest;
        }

        Ok(())
    }
}

fn text(value: &Value) -> String {
    match value {
        Value::Text(text) => text.clone(),
        other => format!("{other:?}"),
    }
}

fn main() -> Result<(), Error> {
    let mut phone = block_on(Database::open(MemoryStore::new(), [1; 32]))?;
    let (group, _) = block_on(phone.found([7; 32]))?;
    let people = ModelId::table(group, [1; 32]);
    let team = ColumnId::text(people, [1; 32]);
    let mut app = App {
        phone,
        group,
        people,
        team,
        mirror: Mirror::default(),
    };
    let person =
        |who: &str, side: &str| Transaction::new().insert(people, who, [(team, Value::from(side))]);

    println!("-- five people");
    app.write(Transaction::new().create_table(people, [(team, "team")]))?;
    for (who, side) in [("ada", "red"), ("bob", "green"), ("cy", "red")] {
        app.write(person(who, side))?;
    }
    app.write(person("dee", "green").insert(people, "eve", [(team, "red".into())]))?;

    println!("-- a mirror begun late reads everything, a part at a time");
    app.begin()?;
    println!(
        "as of checkpoint {:?}: {:?}",
        app.mirror.at,
        app.mirror.count()
    );

    println!("-- from here on it follows what each step reports");
    app.write(person("flo", "blue"))?;

    let read = block_on(app.phone.read(&Query::row(group, people, b"ada")))?;
    let table = Table::read(read.found()).expect("created");
    let ada = table.row(b"ada").expect("inserted");
    app.write(Transaction::new().update(people, ada, [(team, "blue".into())]))?;

    let read = block_on(app.phone.read(&Query::row(group, people, b"bob")))?;
    let table = Table::read(read.found()).expect("created");
    let bob = table.row(b"bob").expect("inserted");
    app.write(Transaction::new().delete_row(people, bob))?;

    // A mirror that was saved at a checkpoint and finds the device
    // settled at the same one has missed nothing. Otherwise it begins
    // again: the library keeps no record of what it reported.
    let all = Query::table(group, people).committed();
    let reached = block_on(app.phone.read_some(&all, 0))?.answer.reached();
    let reached = reached.expect("a committed read");
    assert!(reached.settled && Some(reached.checkpoint) == app.mirror.at);
    assert_eq!(
        app.mirror.count(),
        [("blue", 2), ("green", 1), ("red", 2)].into()
    );
    println!("nothing missed since checkpoint {}", reached.checkpoint);

    Ok(())
}
