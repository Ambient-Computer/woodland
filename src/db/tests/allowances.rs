//! A quantity spent from several devices, and never overspent.

use super::*;
use crate::model::Allowances;

const SEATS: &str = "seats";

fn stock(sim: &Sim) -> ModelId {
    ModelId::allowances(sim.home, [6; 32])
}

fn held(sim: &Sim, device: usize, committed: bool) -> Allowances {
    let query = Query::prefix(sim.home, stock(sim), b"");
    let query = if committed { query.committed() } else { query };

    Allowances::read(&sim.read(device, &query))
}

fn balance(sim: &Sim, device: usize, owner: usize) -> u64 {
    held(sim, device, false).of(SEATS, sim.id(owner)).balance()
}

fn spend(sim: &mut Sim, device: usize, amount: u64) {
    let mine = held(sim, device, false).of(SEATS, sim.id(device));
    sim.write(device, Transaction::new().spend(stock(sim), &mine, amount));
}

fn give(sim: &mut Sim, device: usize, to: usize, amount: u64) {
    let mine = held(sim, device, false).of(SEATS, sim.id(device));
    let to = sim.id(to);
    sim.write(
        device,
        Transaction::new().give(stock(sim), &mine, to, amount),
    );
}

fn accept(sim: &mut Sim, device: usize) {
    let mine = held(sim, device, false).of(SEATS, sim.id(device));
    sim.write(device, Transaction::new().accept(stock(sim), &mine));
}

/// Ten seats: six with the phone, four with the laptop.
fn seated() -> Sim {
    seat(household())
}

fn seat(mut sim: Sim) -> Sim {
    let (model, phone) = (stock(&sim), sim.id(PHONE));
    sim.write(
        PHONE,
        Transaction::new().open_allowance(model, SEATS, 10, phone),
    );
    give(&mut sim, PHONE, LAPTOP, 4);
    sim.settle();
    accept(&mut sim, LAPTOP);
    sim.settle();

    sim
}

#[test]
fn a_quantity_is_split_and_what_was_given_is_spendable_once_accepted() {
    let sim = seated();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let read = held(&sim, device, true);
        assert_eq!(read.total(SEATS), Some(10), "device {device}");
        assert_eq!(read.left(SEATS), 10);
        assert_eq!(read.of(SEATS, sim.id(PHONE)).balance(), 6);
        assert_eq!(read.of(SEATS, sim.id(LAPTOP)).balance(), 4);
        assert_eq!(read.of(SEATS, sim.id(LAPTOP)).offered(), 0);
    }
}

#[test]
fn devices_apart_spend_their_own_and_never_more_than_there_is() {
    let mut sim = seated();
    sim.disconnect_all();

    for _ in 0..3 {
        spend(&mut sim, PHONE, 2);
    }
    for _ in 0..4 {
        spend(&mut sim, LAPTOP, 1);
    }
    assert_eq!(balance(&sim, PHONE, PHONE), 0);
    assert_eq!(balance(&sim, LAPTOP, LAPTOP), 0);

    // There's no more, and each knows it without asking.
    spend(&mut sim, PHONE, 1);
    spend(&mut sim, LAPTOP, 1);
    sim.connect_all();
    sim.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let read = held(&sim, device, true);
        assert_eq!(read.left(SEATS), 0, "device {device}");
        assert_eq!(read.total(SEATS), Some(10));
    }
}

#[test]
fn a_gift_isnt_anyones_to_spend_until_its_accepted() {
    let mut sim = seated();

    give(&mut sim, PHONE, NAS, 5);
    sim.settle();

    let read = held(&sim, NAS, true);
    assert_eq!(read.of(SEATS, sim.id(PHONE)).balance(), 1);
    assert_eq!(read.of(SEATS, sim.id(NAS)).balance(), 0);
    assert_eq!(read.of(SEATS, sim.id(NAS)).offered(), 5);
    assert_eq!(read.left(SEATS), 10);

    spend(&mut sim, NAS, 1);
    sim.settle();
    assert_eq!(held(&sim, PHONE, true).left(SEATS), 10);

    accept(&mut sim, NAS);
    spend(&mut sim, NAS, 1);
    sim.settle();
    let read = held(&sim, PHONE, true);
    assert_eq!(read.of(SEATS, sim.id(NAS)).balance(), 4);
    assert_eq!(read.of(SEATS, sim.id(NAS)).offered(), 0);
    assert_eq!(read.left(SEATS), 9);
}

#[test]
fn several_gifts_are_accepted_together() {
    let mut sim = seated();
    give(&mut sim, PHONE, NAS, 1);
    give(&mut sim, LAPTOP, NAS, 2);
    give(&mut sim, PHONE, NAS, 3);
    sim.settle();

    accept(&mut sim, NAS);
    sim.settle();

    let read = held(&sim, FRIEND, true);
    assert_eq!(read.of(SEATS, sim.id(NAS)).balance(), 6);
    assert_eq!(read.left(SEATS), 10);
}

#[test]
fn nobody_spends_from_anothers_allowance() {
    let mut sim = seated();
    let (model, laptop, nas) = (stock(&sim), sim.id(LAPTOP), sim.id(NAS));

    let theirs = held(&sim, PHONE, false).of(SEATS, laptop);
    sim.write(PHONE, Transaction::new().spend(model, &theirs, 1));
    sim.write(PHONE, Transaction::new().give(model, &theirs, nas, 1));
    sim.write(
        PHONE,
        Transaction::new().open_allowance(model, "tables", 3, nas),
    );
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        let read = held(&sim, device, false);
        assert_eq!(read.of(SEATS, laptop).balance(), 4, "device {device}");
        assert_eq!(read.of(SEATS, nas).offered(), 0);
        assert_eq!(read.total("tables"), None);
    }
}

#[test]
fn more_than_the_allowance_cant_be_spent_or_given() {
    let mut sim = seated();

    spend(&mut sim, LAPTOP, 5);
    give(&mut sim, LAPTOP, NAS, 5);
    sim.settle();

    let read = held(&sim, NAS, true);
    assert_eq!(read.of(SEATS, sim.id(LAPTOP)).balance(), 4);
    assert_eq!(read.left(SEATS), 10);
}

#[test]
fn a_quantity_is_opened_once() {
    let mut sim = household();
    let model = stock(&sim);
    sim.disconnect_all();
    for device in [PHONE, LAPTOP] {
        let me = sim.id(device);
        sim.write(
            device,
            Transaction::new().open_allowance(model, SEATS, 10, me),
        );
    }
    sim.connect_all();
    sim.settle();

    let read = held(&sim, NAS, true);
    assert_eq!(read.total(SEATS), Some(10));
    assert_eq!(read.left(SEATS), 10);
}

/// The laptop accepts a gift and spends out of it, all before anyone
/// has agreed the gift was given. It wasn't: giving it was part of a
/// transaction that lost a claim. So the spend doesn't happen either.
#[test]
fn what_was_spent_out_of_a_gift_that_wasnt_given_doesnt_happen() {
    // Four contributors, so that two of them can't settle anything.
    let mut sim = seat(Sim::group(4, 4).0);
    let (model, names) = (stock(&sim), usernames(&sim));
    for _ in 0..4 {
        spend(&mut sim, LAPTOP, 1);
    }
    sim.settle();
    sim.disconnect_all();

    // The NAS takes a name the phone is about to want, and the phone
    // writes more first, so that its claim is placed after.
    sim.write(NAS, Transaction::new().claim(names, "host", "nas"));
    write(&mut sim, PHONE, "busy", "yes");
    let mine = held(&sim, PHONE, false).of(SEATS, sim.id(PHONE));
    let laptop = sim.id(LAPTOP);
    sim.write(PHONE, {
        let transaction = Transaction::new().claim(names, "host", "phone");

        transaction.give(model, &mine, laptop, 3)
    });
    sim.connect(PHONE, LAPTOP);
    sim.settle();
    sim.disconnect_all();
    assert_eq!(held(&sim, LAPTOP, true).of(SEATS, laptop).offered(), 0);
    accept(&mut sim, LAPTOP);
    spend(&mut sim, LAPTOP, 2);
    assert_eq!(balance(&sim, LAPTOP, LAPTOP), 1);

    // Everyone hears of the claim before enough of them can meet to
    // settle anything, so whatever is settled has both claims in it.
    for device in [PHONE, LAPTOP] {
        sim.connect(NAS, device);
        sim.settle();
        sim.disconnect_all();
    }
    assert_eq!(balance(&sim, LAPTOP, LAPTOP), 0);
    sim.connect_all();
    sim.settle();
    let host = sim.committed(NAS, sim.home, names, "host");
    assert_eq!(host.as_deref(), Some("nas"));

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let read = held(&sim, device, true);
        assert_eq!(
            read.of(SEATS, sim.id(PHONE)).balance(),
            6,
            "device {device}"
        );
        assert_eq!(read.of(SEATS, sim.id(LAPTOP)).balance(), 0);
        assert_eq!(read.left(SEATS), 6);
    }
}
