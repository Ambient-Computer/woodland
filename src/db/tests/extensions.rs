//! A kind of model the application adds: a register of who holds
//! what, where a thing is taken only if it's free and handed on only
//! by whoever the author thinks has it.

use super::*;
use crate::{Declared, Extension, Needs, extend};

const LOANS: u8 = 3;

/// `take thing who`, `hand thing from to`, `return thing`. Anything
/// else isn't an operation of this kind.
fn declare(operation: &[u8]) -> Option<Declared> {
    let text = std::str::from_utf8(operation).ok()?;
    let words: Vec<&str> = text.split(' ').collect();
    let key = |thing: &str| thing.as_bytes().to_vec();
    let value = |who: &str| Some(who.as_bytes().to_vec());
    let declared = match words[..] {
        ["take", thing, who] => Declared {
            requires: vec![(key(thing), Needs::Vacant)],
            writes: vec![(key(thing), value(who))],
        },
        ["hand", thing, from, to] => Declared {
            requires: vec![(key(thing), Needs::Is(from.as_bytes().to_vec()))],
            writes: vec![(key(thing), value(to))],
        },
        ["return", thing] => Declared {
            requires: vec![(key(thing), Needs::Present)],
            writes: vec![(key(thing), None)],
        },
        _ => return None,
    };

    Some(declared)
}

/// A household whose application has the kind.
fn lending() -> (Sim, GroupId, ModelId) {
    let _ = extend(Extension {
        number: LOANS,
        declare,
        combine: None,
    });
    let (sim, group) = Sim::group(4, 3);
    let loans = ModelId::of_kind(LOANS, group, [1; 32]).expect("a number that's allowed");

    (sim, group, loans)
}

fn loan(sim: &mut Sim, device: usize, loans: ModelId, operation: &str) {
    sim.write(device, Transaction::new().change(loans, operation));
}

#[test]
fn an_applications_kind_is_written_and_read_like_any_other() {
    let (mut sim, group, loans) = lending();
    loan(&mut sim, PHONE, loans, "take drill ada");
    assert_eq!(
        sim.shown(PHONE, group, loans, "drill").as_deref(),
        Some("ada")
    );
    assert_eq!(sim.committed(PHONE, group, loans, "drill"), None);
    sim.settle();
    loan(&mut sim, LAPTOP, loans, "hand drill ada bo");
    loan(&mut sim, LAPTOP, loans, "take ladder cy");
    sim.settle();
    loan(&mut sim, NAS, loans, "return ladder");
    sim.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let drill = sim.committed(device, group, loans, "drill");
        assert_eq!(drill.as_deref(), Some("bo"), "device {device}");
        assert_eq!(sim.committed(device, group, loans, "ladder"), None);
    }

    // And it's there after a restart, with the kind in place.
    sim.restart(FRIEND);
    let drill = sim.committed(FRIEND, group, loans, "drill");
    assert_eq!(drill.as_deref(), Some("bo"));
}

/// What the kind says an operation requires decides which of two
/// happens, the same way everywhere.
#[test]
fn what_an_applications_kind_requires_is_what_prunes() {
    let (mut sim, group, loans) = lending();
    sim.disconnect_all();
    loan(&mut sim, PHONE, loans, "take drill ada");
    loan(&mut sim, LAPTOP, loans, "take drill bo");
    sim.connect_all();
    sim.settle();
    let winner = sim
        .committed(PHONE, group, loans, "drill")
        .expect("one of them");

    // Handed on by someone who's wrong about who has it: pruned, and
    // what's in the transaction with it too.
    let loser = if winner == "ada" { "bo" } else { "ada" };
    let hand = format!("hand drill {loser} cy");
    let transaction = Transaction::new().change(loans, hand);
    sim.write(NAS, transaction.set(notes(group), "drill", "lent to cy"));
    loan(&mut sim, NAS, loans, "return ladder");
    sim.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let drill = sim.committed(device, group, loans, "drill");
        assert_eq!(drill.as_deref(), Some(winner.as_str()), "device {device}");
        assert_eq!(sim.committed(device, group, notes(group), "drill"), None);
        assert_eq!(sim.committed(device, group, loans, "ladder"), None);
    }
}

/// An operation the kind doesn't know, or one for another kind, takes
/// its transaction with it.
#[test]
fn what_isnt_an_operation_of_the_kind_doesnt_happen() {
    let (mut sim, group, loans) = lending();
    let nonsense = Transaction::new().change(loans, "borrow drill");
    sim.write(PHONE, nonsense.set(notes(group), "a", "1"));
    sim.write(PHONE, set(loans, "drill", "ada"));
    let transaction = Transaction::new().change(notes(group), "take drill ada");
    sim.write(PHONE, transaction.set(notes(group), "b", "2"));
    sim.settle();

    assert_eq!(sim.shown(PHONE, group, loans, "drill"), None);
    assert_eq!(sim.shown(PHONE, group, notes(group), "a"), None);
    assert_eq!(sim.shown(PHONE, group, notes(group), "b"), None);
}

/// What any model can do, one of an application's kind can.
#[test]
fn an_applications_kind_can_be_conditional_held_back_and_dropped() {
    let (mut sim, group, loans) = lending();
    let held = loans.committed_only();
    loan(&mut sim, PHONE, held, "take drill ada");
    assert_eq!(sim.shown(PHONE, group, held, "drill"), None);
    sim.settle();
    assert_eq!(
        sim.shown(PHONE, group, held, "drill").as_deref(),
        Some("ada")
    );

    loan(&mut sim, PHONE, loans, "take saw bo");
    sim.settle();
    let expect = Transaction::new().expect(loans, "saw", "cy");
    sim.write(LAPTOP, expect.change(loans, "take ladder cy"));
    let expect = Transaction::new().expect(loans, "saw", "bo");
    sim.write(LAPTOP, expect.change(loans, "take plane cy"));
    sim.settle();
    assert_eq!(sim.committed(NAS, group, loans, "ladder"), None);
    assert_eq!(
        sim.committed(NAS, group, loans, "plane").as_deref(),
        Some("cy")
    );

    sim.write(PHONE, Transaction::new().drop_model(loans));
    sim.settle();
    loan(&mut sim, PHONE, loans, "take chisel ada");
    sim.settle();
    for key in ["saw", "plane", "chisel"] {
        assert_eq!(sim.committed(NAS, group, loans, key), None, "{key}");
    }
}

#[test]
fn an_application_has_so_many_kinds_and_each_number_once() {
    let taken = extend(Extension {
        number: 7,
        declare,
        combine: None,
    });
    let again = extend(Extension {
        number: 7,
        declare,
        combine: None,
    });
    let beyond = extend(Extension {
        number: 64,
        declare,
        combine: None,
    });
    assert_eq!((taken, again, beyond), (true, false, false));

    let group = GroupId::from_bytes([3; 32]);
    assert!(ModelId::of_kind(63, group, [1; 32]).is_some());
    assert!(ModelId::of_kind(64, group, [1; 32]).is_none());
    let one = ModelId::of_kind(7, group, [1; 32]);
    assert_ne!(one, ModelId::of_kind(8, group, [1; 32]));
    assert_ne!(one, ModelId::of_kind(7, group, [2; 32]));
}

/// The NAS runs an application without the kind. It keeps what it
/// can't read and passes it on, takes the others' word that it's
/// committed, and reads it once its application has the kind.
#[test]
fn a_device_without_the_kind_carries_it_and_catches_up() {
    let (mut sim, group, loans) = lending();
    sim.plain.insert(NAS);
    sim.restart(NAS);
    sim.disconnect_all();
    sim.connect(PHONE, NAS);
    sim.connect(NAS, FRIEND);
    loan(&mut sim, PHONE, loans, "take drill ada");
    sim.run();

    assert_eq!(sim.shown(NAS, group, loans, "drill"), None);
    assert_eq!(sim.entries_kept(NAS, group), 1);
    assert_eq!(
        sim.shown(FRIEND, group, loans, "drill").as_deref(),
        Some("ada")
    );
    let written =
        block_on(sim.devices[NAS].write(group, Transaction::new().change(loans, "take saw bo")));
    assert!(matches!(written, Err(Error::Unreadable)));

    sim.connect_all();
    sim.settle();
    let folded = |sim: &Sim, device: usize| sim.devices[device].groups[&group].folded();
    assert_eq!(
        sim.committed(PHONE, group, loans, "drill").as_deref(),
        Some("ada")
    );
    assert_eq!(sim.committed(NAS, group, loans, "drill"), None);
    assert_eq!(folded(&sim, NAS), folded(&sim, PHONE));

    // Starting again without the kind changes nothing.
    sim.restart(NAS);
    sim.connect_all();
    sim.settle();
    assert_eq!(sim.committed(NAS, group, loans, "drill"), None);

    sim.plain.remove(&NAS);
    sim.restart(NAS);
    sim.connect_all();
    sim.settle();
    assert_eq!(
        sim.committed(NAS, group, loans, "drill").as_deref(),
        Some("ada")
    );
    loan(&mut sim, NAS, loans, "hand drill ada bo");
    sim.settle();
    assert_eq!(
        sim.committed(PHONE, group, loans, "drill").as_deref(),
        Some("bo")
    );
}

const TALLIES: u8 = 4;

/// `add key n`: the key counts everything added to it.
fn tally(operation: &[u8]) -> Option<Declared> {
    let text = std::str::from_utf8(operation).ok()?;
    let ["add", key, count] = text.split(' ').collect::<Vec<_>>()[..] else {
        return None;
    };
    count.parse::<u64>().ok()?;

    Some(Declared {
        requires: Vec::new(),
        writes: vec![(key.as_bytes().to_vec(), Some(count.as_bytes().to_vec()))],
    })
}

fn sum(held: Option<&[u8]>, write: &[u8]) -> Vec<u8> {
    let number = |bytes: &[u8]| std::str::from_utf8(bytes).ok()?.parse::<u64>().ok();
    let held = held.and_then(number).unwrap_or(0);
    let sum = held.saturating_add(number(write).unwrap_or(0));

    sum.to_string().into_bytes()
}

/// A kind can say how writes to one key combine. Then none of them
/// displaces another: both devices' count.
#[test]
fn an_applications_kind_can_merge() {
    let _ = extend(Extension {
        number: TALLIES,
        declare: tally,
        combine: Some(sum),
    });
    let (mut sim, group) = Sim::group(4, 3);
    let tallies = ModelId::of_kind(TALLIES, group, [1; 32]).expect("a number that's allowed");
    let add = |count: u64| Transaction::new().change(tallies, format!("add visits {count}"));
    sim.write(PHONE, add(1));
    sim.settle();

    sim.disconnect_all();
    sim.write(PHONE, add(2).change(tallies, "add visits 10"));
    sim.write(PHONE, add(4));
    sim.write(LAPTOP, add(100));
    assert_eq!(
        sim.shown(PHONE, group, tallies, "visits").as_deref(),
        Some("17")
    );
    assert_eq!(
        sim.shown(LAPTOP, group, tallies, "visits").as_deref(),
        Some("101")
    );
    assert_eq!(
        sim.committed(PHONE, group, tallies, "visits").as_deref(),
        Some("1")
    );

    sim.connect_all();
    sim.settle();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let visits = sim.committed(device, group, tallies, "visits");
        assert_eq!(visits.as_deref(), Some("117"), "device {device}");
        let visits = sim.shown(device, group, tallies, "visits");
        assert_eq!(visits.as_deref(), Some("117"), "device {device}");
    }
}
