//! What an application is told of its own transactions.

use super::*;

/// Everything `device` has been told about the last thing it wrote.
fn told(sim: &Sim, device: usize) -> Vec<Outcome> {
    let wrote = sim.wrote[device].expect("it wrote something");
    let about = sim.told[device].iter().filter(|(id, _)| *id == wrote);

    about.map(|(_, outcome)| *outcome).collect()
}

#[test]
fn an_author_is_told_when_its_transaction_counts_and_when_its_committed() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.write(PHONE, set(notes(group), "todo", "milk"));
    let wrote = sim.wrote[PHONE].expect("named");
    assert_eq!(wrote.author, sim.id(PHONE));
    assert_eq!(told(&sim, PHONE), [Outcome::Live]);

    sim.settle();

    assert_eq!(told(&sim, PHONE), [Outcome::Live, Outcome::Committed]);
    // Nobody else is told anything: it wasn't theirs.
    assert!(sim.told[LAPTOP].is_empty() && sim.told[NAS].is_empty());

    // Each thing it writes has its own name.
    sim.write(PHONE, set(notes(group), "todo", "eggs"));
    assert_ne!(sim.wrote[PHONE], Some(wrote));
    assert_eq!(told(&sim, PHONE), [Outcome::Live]);
}

#[test]
fn an_author_is_told_when_its_transaction_stops_counting() {
    let mut cluster = household();
    cluster.disconnect_all();
    guards::count(&mut cluster, PHONE, None, "1");
    guards::count(&mut cluster, LAPTOP, None, "2");
    assert_eq!(told(&cluster, PHONE), [Outcome::Live]);
    assert_eq!(told(&cluster, LAPTOP), [Outcome::Live]);

    // One of them got there first. The other finds out.
    cluster.connect_all();
    cluster.run();
    let (won, lost) = match read(&cluster, PHONE, "count").as_deref() {
        Some("1") => (PHONE, LAPTOP),
        _ => (LAPTOP, PHONE),
    };
    assert_eq!(told(&cluster, won), [Outcome::Live]);
    assert_eq!(told(&cluster, lost), [Outcome::Live, Outcome::Pruned]);

    cluster.settle();

    assert_eq!(told(&cluster, won), [Outcome::Live, Outcome::Committed]);
    assert_eq!(
        told(&cluster, lost),
        [Outcome::Live, Outcome::Pruned, Outcome::Rejected]
    );
}

#[test]
fn an_author_is_told_at_once_of_a_transaction_that_doesnt_count() {
    let mut cluster = household();
    guards::count(&mut cluster, PHONE, Some("7"), "8");
    assert_eq!(told(&cluster, PHONE), [Outcome::Pruned]);

    cluster.settle();

    assert_eq!(told(&cluster, PHONE), [Outcome::Pruned, Outcome::Rejected]);
}

#[test]
fn an_author_is_told_when_its_transaction_never_will_be_committed() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.isolate(NAS);
    sim.write(NAS, set(notes(group), "from the nas", "x"));
    sim.reconfigure(PHONE, group, [PHONE, LAPTOP]);
    sim.settle();
    assert_eq!(told(&sim, NAS), [Outcome::Live]);

    sim.connect_all();
    sim.settle();

    assert_eq!(told(&sim, NAS), [Outcome::Live, Outcome::Discarded]);
}

/// A device that takes a snapshot never commits what the snapshot
/// covers, so it can't say what came of it.
#[test]
fn an_author_that_catches_up_from_a_snapshot_is_told_it_was_covered() {
    let (mut sim, group) = Sim::group(3, 3);
    sim.write(NAS, set(notes(group), "from the nas", "x"));
    sim.run();
    sim.isolate(NAS);
    sim.write(PHONE, set(notes(group), "todo", "milk"));
    sim.settle();
    sim.compact(PHONE, group);
    sim.compact(LAPTOP, group);
    assert_eq!(told(&sim, NAS), [Outcome::Live]);

    sim.connect_all();
    sim.settle();

    assert!(
        sim.committed(NAS, group, notes(group), "from the nas")
            .is_some()
    );
    assert_eq!(told(&sim, NAS), [Outcome::Live, Outcome::Covered]);
}
