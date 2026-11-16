//! One device, no network, no account.
//!
//! ```sh
//! cargo run --example alone
//! ```
//!
//! A group is where data lives. A device founds one and is its only
//! contributor. It writes, reads what it wrote at once, and commits by
//! itself: a quorum of one.

#![expect(clippy::print_stdout, reason = "an example says what it did")]

#[path = "support/executor.rs"]
mod executor;

use executor::block_on;
use woodland::{Answer, Database, Error, MemoryStore, ModelId, Outcome, Query, Transaction};

fn main() -> Result<(), Error> {
    // The secret is the device's signing key. It's made once, and
    // kept in the store from then on: the store is the device.
    let mut phone = block_on(Database::open(MemoryStore::new(), [1; 32]))?;

    // Every call that changes anything is one step: one atomic batch
    // to the store, and then what the application should do about it.
    // Alone, there's nobody to send anything to.
    let (group, outputs) = block_on(phone.found([7; 32]))?;
    assert_eq!(outputs.sends.len(), 0);

    // A model is one kind of data in the group. This one is a map
    // from keys to values, where the last write wins. Naming a model
    // is all it takes to have one.
    let notes = ModelId::kv(group, [1; 32]);
    let write = Transaction::new().set(notes, "todo", "milk");
    let outputs = block_on(phone.write(group, write))?;
    let id = outputs.wrote.expect("a write says what it wrote");
    println!("wrote transaction {} of its chain", id.seq);

    // A read never waits on anyone. By default it's optimistic: it
    // shows everything this device knows of, and may be revised.
    let todo = Query::key(group, notes, b"todo");
    let answer = block_on(phone.read(&todo))?;
    assert!(matches!(answer, Answer::Optimistic(_)));
    println!("optimistic: {:?}", text(&answer));

    // A committed read shows only what a checkpoint covers. Nothing
    // has been committed yet.
    let answer = block_on(phone.read(&todo.clone().committed()))?;
    println!("committed, before a checkpoint: {:?}", text(&answer));

    // Checkpoints are attempted when the device is told time has
    // passed. With a quorum of one, the attempt succeeds by itself.
    // A step does a bounded amount of work, and says when there's more.
    let mut told = Vec::new();
    let mut outputs = block_on(phone.tick())?;
    loop {
        told.extend(outputs.outcomes.iter().map(|(_, outcome)| *outcome));
        if !outputs.unfinished {
            break;
        }
        outputs = block_on(phone.resume())?;
    }

    // The device is told what became of what it wrote.
    assert!(told.contains(&Outcome::Committed));
    let answer = block_on(phone.read(&todo.committed()))?;
    assert!(answer.is_committed());
    println!("committed, after one: {:?}", text(&answer));

    Ok(())
}

fn text(answer: &Answer) -> Option<String> {
    let (_, value) = answer.found().first()?;

    Some(String::from_utf8_lossy(value).into_owned())
}
