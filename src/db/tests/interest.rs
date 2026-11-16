//! A device that holds only the clocks it wants.

use super::*;

fn photos(group: GroupId) -> ClockId {
    ClockId::new(group, [7; 32])
}

fn captions(group: GroupId) -> ModelId {
    ModelId::kv(photos(group), [1; 32])
}

fn want(
    sim: &mut Sim,
    device: usize,
    group: GroupId,
    clocks: Option<&[ClockId]>,
) -> Result<(), Error> {
    let clocks = clocks.map(|it| it.iter().copied().collect());
    let outputs = block_on(sim.devices[device].want(group, clocks))?;
    sim.absorb(device, outputs);

    Ok(())
}

/// How many keys the device stores for `clock`: entries, and values
/// committed or not.
fn kept(sim: &Sim, device: usize, group: GroupId, clock: ClockId) -> usize {
    let view = sim.devices[device].store.view();
    let prefixes = [
        keys::logs(group),
        keys::committed(group),
        keys::uncommitted(group),
    ];
    let count = |prefix: Vec<u8>| {
        let (start, end) = keys::range([&prefix[..], clock.as_bytes()].concat());

        block_on(crate::db::step::scan(&view, &start, &end))
            .expect("scanned")
            .len()
    };

    prefixes.into_iter().map(count).sum()
}

fn folded(sim: &Sim, device: usize, group: GroupId) -> u64 {
    sim.devices[device].groups[&group].folded()
}

/// Three devices decide. The friend follows along, with no role.
fn followed() -> (Sim, GroupId) {
    let (mut sim, group) = Sim::group(4, 3);
    sim.settle();

    (sim, group)
}

#[test]
fn a_device_is_sent_and_keeps_only_the_clocks_it_wants() {
    let (mut sim, group) = followed();
    want(&mut sim, FRIEND, group, Some(&[photos(group)])).expect("wanted");

    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.write_to(LAPTOP, group, set(captions(group), "beach", "sunset"));
    sim.settle();

    let caption = sim.committed(FRIEND, group, captions(group), "beach");
    assert_eq!(caption.as_deref(), Some("sunset"));
    assert_eq!(sim.committed(FRIEND, group, notes(group), "todo"), None);
    assert_eq!(sim.shown(FRIEND, group, notes(group), "todo"), None);
    assert_eq!(kept(&sim, FRIEND, group, group.into()), 0);
    assert!(kept(&sim, NAS, group, group.into()) > 0);

    // It keeps up with the chain all the same.
    assert_eq!(folded(&sim, FRIEND, group), folded(&sim, NAS, group));
}

#[test]
fn a_device_lets_go_of_a_clock_it_stops_wanting() {
    let (mut sim, group) = followed();
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.write_to(PHONE, group, set(captions(group), "beach", "sunset"));
    sim.settle();
    sim.disconnect_all();
    sim.write_to(PHONE, group, set(notes(group), "todo", "eggs"));
    sim.connect(PHONE, FRIEND);
    sim.run();
    assert!(kept(&sim, FRIEND, group, group.into()) > 0);

    want(&mut sim, FRIEND, group, Some(&[photos(group)])).expect("wanted");

    assert_eq!(kept(&sim, FRIEND, group, group.into()), 0);
    assert_eq!(sim.shown(FRIEND, group, notes(group), "todo"), None);
    let caption = sim.committed(FRIEND, group, captions(group), "beach");
    assert_eq!(caption.as_deref(), Some("sunset"));

    // And it stays gone, across a restart and more sync.
    sim.restart(FRIEND);
    sim.connect_all();
    sim.write_to(LAPTOP, group, set(captions(group), "hill", "fog"));
    sim.settle();
    assert_eq!(kept(&sim, FRIEND, group, group.into()), 0);
    let caption = sim.committed(FRIEND, group, captions(group), "hill");
    assert_eq!(caption.as_deref(), Some("fog"));
    assert_eq!(folded(&sim, FRIEND, group), folded(&sim, NAS, group));
}

#[test]
fn a_device_with_a_say_holds_everything() {
    let (mut sim, group) = followed();

    let refused = want(&mut sim, PHONE, group, Some(&[photos(group)]));

    assert!(matches!(refused, Err(Error::Votes)));
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();
    let note = sim.committed(PHONE, group, notes(group), "todo");
    assert_eq!(note.as_deref(), Some("milk"));
}

#[test]
fn a_device_can_want_more_until_something_is_committed_without_it() {
    let (mut sim, group) = followed();
    want(&mut sim, FRIEND, group, Some(&[photos(group)])).expect("wanted");
    sim.write_to(PHONE, group, set(captions(group), "beach", "sunset"));
    sim.settle();

    // Nothing has been committed to the clock it left out.
    want(&mut sim, FRIEND, group, None).expect("wanted");
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();
    let note = sim.committed(FRIEND, group, notes(group), "todo");
    assert_eq!(note.as_deref(), Some("milk"));

    // Now something has, and letting go of it can't be undone.
    want(&mut sim, FRIEND, group, Some(&[photos(group)])).expect("wanted");
    let refused = want(&mut sim, FRIEND, group, None);
    assert!(matches!(refused, Err(Error::PassedOver)));
    let refused = want(&mut sim, FRIEND, group, Some(&[group.into()]));
    assert!(matches!(refused, Err(Error::PassedOver)));
    want(&mut sim, FRIEND, group, Some(&[])).expect("less is fine");
}

#[test]
fn a_checkpoint_passed_without_a_clock_cant_be_made_up_for() {
    let (mut sim, group) = followed();
    want(&mut sim, FRIEND, group, Some(&[photos(group)])).expect("wanted");
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();

    let refused = want(&mut sim, FRIEND, group, None);

    assert!(matches!(refused, Err(Error::PassedOver)));
}

/// The others have shed their history, so the newcomer gets state, and
/// only of the clock it asked for.
#[test]
fn a_newcomer_catches_up_on_only_the_clocks_it_wants() {
    let (mut sim, group) = followed();
    for index in 0..300 {
        let key = format!("note {index}");
        sim.write_to(PHONE, group, set(notes(group), &key, "x"));
    }
    for index in 0..300 {
        let key = format!("photo {index}");
        sim.write_to(LAPTOP, group, set(captions(group), &key, "y"));
    }
    sim.settle();
    for device in [PHONE, LAPTOP, NAS] {
        sim.compact(device, group);
    }

    sim.rejoin(FRIEND, group, PHONE);
    want(&mut sim, FRIEND, group, Some(&[photos(group)])).expect("wanted");
    sim.connect_all();
    sim.settle();

    assert_eq!(folded(&sim, FRIEND, group), folded(&sim, NAS, group));
    let caption = sim.committed(FRIEND, group, captions(group), "photo 299");
    assert_eq!(caption.as_deref(), Some("y"));
    assert_eq!(kept(&sim, FRIEND, group, group.into()), 0);
    assert_eq!(kept(&sim, FRIEND, group, photos(group)), 300);

    let refused = want(&mut sim, FRIEND, group, None);
    assert!(matches!(refused, Err(Error::PassedOver)));

    // What comes after arrives as usual.
    sim.write_to(NAS, group, set(captions(group), "late", "z"));
    sim.write_to(NAS, group, set(notes(group), "late", "z"));
    sim.settle();
    let caption = sim.committed(FRIEND, group, captions(group), "late");
    assert_eq!(caption.as_deref(), Some("z"));
    assert_eq!(kept(&sim, FRIEND, group, group.into()), 0);
}

const GUEST: usize = 4;

/// A device with part of the state can't bring another up to date on
/// all of it, and doesn't try. One that has it all does.
#[test]
fn a_device_holding_some_clocks_gives_state_only_for_those() {
    let (mut sim, group) = Sim::group(5, 3);
    sim.settle();
    want(&mut sim, FRIEND, group, Some(&[photos(group)])).expect("wanted");
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.write_to(PHONE, group, set(captions(group), "beach", "sunset"));
    sim.settle();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        sim.compact(device, group);
    }

    sim.rejoin(GUEST, group, PHONE);
    sim.connect(GUEST, FRIEND);
    sim.settle();
    assert!(folded(&sim, GUEST, group) < folded(&sim, NAS, group));
    assert_eq!(sim.committed(GUEST, group, captions(group), "beach"), None);

    sim.connect(GUEST, NAS);
    sim.settle();
    assert_eq!(folded(&sim, GUEST, group), folded(&sim, NAS, group));
    let note = sim.committed(GUEST, group, notes(group), "todo");
    assert_eq!(note.as_deref(), Some("milk"));

    // The same guest, wanting what the friend has, gets it from the
    // friend.
    sim.rejoin(GUEST, group, PHONE);
    want(&mut sim, GUEST, group, Some(&[photos(group)])).expect("wanted");
    sim.connect(GUEST, FRIEND);
    sim.settle();
    assert_eq!(folded(&sim, GUEST, group), folded(&sim, NAS, group));
    let caption = sim.committed(GUEST, group, captions(group), "beach");
    assert_eq!(caption.as_deref(), Some("sunset"));
}

#[test]
fn a_writer_with_no_say_writes_only_to_clocks_it_holds() {
    let (mut sim, group) = Sim::group(4, 3);
    let roles = [
        (PHONE, Role::contributor(1)),
        (LAPTOP, Role::contributor(1)),
        (NAS, Role::contributor(1)),
        (FRIEND, Role::contributor(0)),
    ];
    sim.assign(PHONE, group, roles);
    sim.settle();
    want(&mut sim, FRIEND, group, Some(&[photos(group)])).expect("wanted");

    let refused = block_on(sim.devices[FRIEND].write(group, set(notes(group), "a", "1")));
    assert!(matches!(refused, Err(Error::NotAContributor)));

    // Alone, it starts no checkpoint: it couldn't say what one covers.
    sim.disconnect_all();
    sim.write_to(FRIEND, group, set(captions(group), "beach", "sunset"));
    for _ in 0..32 {
        sim.tick(FRIEND);
    }
    assert!(!sim.devices[FRIEND].groups[&group].attempting());

    sim.connect_all();
    sim.settle();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        let caption = sim.committed(device, group, captions(group), "beach");
        assert_eq!(caption.as_deref(), Some("sunset"), "device {device}");
    }
}

/// Delivers everything, and returns how many bytes went to `device`.
fn carried(sim: &mut Sim, device: usize) -> usize {
    let mut bytes = 0;
    while let Some((_, to, message)) = sim.queue.front() {
        if *to == device {
            bytes += message.len();
        }
        sim.deliver();
    }

    bytes
}

const LONG: usize = 20_000;

fn long() -> String {
    "x".repeat(LONG)
}

#[test]
fn what_a_device_doesnt_want_isnt_sent_to_it() {
    let (mut sim, group) = followed();
    want(&mut sim, FRIEND, group, Some(&[photos(group)])).expect("wanted");
    sim.settle();
    sim.disconnect_all();
    sim.connect(PHONE, FRIEND);
    sim.run();

    sim.write_to(PHONE, group, set(notes(group), "essay", &long()));
    assert!(carried(&mut sim, FRIEND) < LONG);
    sim.tick(PHONE);
    assert!(carried(&mut sim, FRIEND) < LONG);

    sim.write_to(PHONE, group, set(captions(group), "essay", &long()));
    assert!(carried(&mut sim, FRIEND) > LONG);
}

#[test]
fn history_a_newcomer_doesnt_want_isnt_sent_to_it() {
    let (mut sim, group) = followed();
    sim.write_to(PHONE, group, set(notes(group), "essay", &long()));
    sim.write_to(PHONE, group, set(captions(group), "beach", "sunset"));
    sim.settle();

    sim.rejoin(FRIEND, group, PHONE);
    want(&mut sim, FRIEND, group, Some(&[photos(group)])).expect("wanted");
    sim.connect(PHONE, FRIEND);
    let mut bytes = carried(&mut sim, FRIEND);
    for _ in 0..4 {
        sim.tick(PHONE);
        sim.tick(FRIEND);
        bytes += carried(&mut sim, FRIEND);
    }

    assert!(bytes < LONG);
    let caption = sim.committed(FRIEND, group, captions(group), "beach");
    assert_eq!(caption.as_deref(), Some("sunset"));
}

#[test]
fn state_a_newcomer_doesnt_want_isnt_sent_to_it() {
    let (mut sim, group) = followed();
    sim.write_to(PHONE, group, set(notes(group), "essay", &long()));
    sim.write_to(PHONE, group, set(captions(group), "beach", "sunset"));
    sim.settle();
    sim.compact(PHONE, group);

    sim.rejoin(FRIEND, group, PHONE);
    want(&mut sim, FRIEND, group, Some(&[photos(group)])).expect("wanted");
    sim.connect(PHONE, FRIEND);
    let mut bytes = carried(&mut sim, FRIEND);
    for _ in 0..4 {
        sim.tick(PHONE);
        sim.tick(FRIEND);
        bytes += carried(&mut sim, FRIEND);
    }

    assert!(bytes < LONG);
    let caption = sim.committed(FRIEND, group, captions(group), "beach");
    assert_eq!(caption.as_deref(), Some("sunset"));
}

#[test]
fn a_device_holding_some_clocks_doesnt_offer_state_it_cant_complete() {
    let (mut sim, group) = Sim::group(5, 3);
    sim.settle();
    want(&mut sim, FRIEND, group, Some(&[photos(group)])).expect("wanted");
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.write_to(PHONE, group, set(captions(group), "essay", &long()));
    sim.settle();
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        sim.compact(device, group);
    }

    sim.rejoin(GUEST, group, PHONE);
    sim.connect(GUEST, FRIEND);
    let mut bytes = carried(&mut sim, GUEST);
    for _ in 0..4 {
        sim.tick(FRIEND);
        sim.tick(GUEST);
        bytes += carried(&mut sim, GUEST);
    }

    assert!(bytes < LONG);
}

fn ask(sim: &mut Sim, device: usize, peer: usize, group: GroupId, clocks: Option<&[ClockId]>) {
    let clocks = clocks.map(|it| it.iter().copied().collect());
    let peer = sim.id(peer);
    let outputs = block_on(sim.devices[device].ask(peer, group, clocks)).expect("asked");
    sim.absorb(device, outputs);
    sim.run();
}

/// The friend reaches the phone over a connection it wants little of.
/// The rest waits for another connection, and so does committing.
#[test]
fn a_connection_carries_only_the_clocks_asked_of_it() {
    let (mut sim, group) = followed();
    sim.isolate(FRIEND);
    sim.connect(FRIEND, PHONE);
    ask(&mut sim, FRIEND, PHONE, group, Some(&[photos(group)]));
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.write_to(PHONE, group, set(captions(group), "beach", "sunset"));
    sim.settle();

    let caption = sim.shown(FRIEND, group, captions(group), "beach");
    assert_eq!(caption.as_deref(), Some("sunset"));
    assert_eq!(sim.shown(FRIEND, group, notes(group), "todo"), None);
    assert_eq!(sim.committed(FRIEND, group, captions(group), "beach"), None);
    assert!(folded(&sim, FRIEND, group) < folded(&sim, PHONE, group));

    // It still holds the clock, and takes it from anyone else.
    sim.connect(FRIEND, LAPTOP);
    sim.settle();
    let todo = sim.committed(FRIEND, group, notes(group), "todo");
    assert_eq!(todo.as_deref(), Some("milk"));
    assert_eq!(folded(&sim, FRIEND, group), folded(&sim, PHONE, group));
}

#[test]
fn what_a_connection_is_asked_for_can_change() {
    let (mut sim, group) = followed();
    sim.isolate(FRIEND);
    sim.connect(FRIEND, PHONE);
    ask(&mut sim, FRIEND, PHONE, group, Some(&[]));
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();
    assert_eq!(sim.shown(FRIEND, group, notes(group), "todo"), None);

    ask(&mut sim, FRIEND, PHONE, group, None);
    sim.settle();
    let todo = sim.committed(FRIEND, group, notes(group), "todo");
    assert_eq!(todo.as_deref(), Some("milk"));
}

#[test]
fn what_was_asked_of_a_connection_ends_with_it() {
    let (mut sim, group) = followed();
    sim.isolate(FRIEND);
    sim.connect(FRIEND, PHONE);
    ask(&mut sim, FRIEND, PHONE, group, Some(&[]));
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.settle();
    assert_eq!(sim.shown(FRIEND, group, notes(group), "todo"), None);

    sim.disconnect(FRIEND, PHONE);
    sim.connect(FRIEND, PHONE);
    sim.settle();
    let todo = sim.committed(FRIEND, group, notes(group), "todo");
    assert_eq!(todo.as_deref(), Some("milk"));

    // Asking a peer that isn't there asks nothing.
    sim.disconnect(FRIEND, PHONE);
    ask(&mut sim, FRIEND, PHONE, group, Some(&[]));
    let unknown = GroupId::from_bytes([3; 32]);
    let peer = sim.id(PHONE);
    let asked = block_on(sim.devices[FRIEND].ask(peer, unknown, None));
    assert!(matches!(asked, Err(Error::UnknownGroup)));
    sim.connect(FRIEND, PHONE);
    sim.write_to(PHONE, group, set(notes(group), "todo", "eggs"));
    sim.settle();
    let todo = sim.committed(FRIEND, group, notes(group), "todo");
    assert_eq!(todo.as_deref(), Some("eggs"));
}

/// Committed state stands in for history a peer has let go of, and
/// it's the state of everything the device holds or it's no use.
#[test]
fn a_device_far_behind_is_sent_whole_state_whatever_it_asked() {
    let (mut sim, group) = followed();
    sim.isolate(FRIEND);
    sim.write_to(PHONE, group, set(notes(group), "todo", "milk"));
    sim.write_to(PHONE, group, set(captions(group), "beach", "sunset"));
    sim.settle();
    sim.compact(PHONE, group);

    sim.connect(FRIEND, PHONE);
    ask(&mut sim, FRIEND, PHONE, group, Some(&[photos(group)]));
    sim.settle();

    let todo = sim.committed(FRIEND, group, notes(group), "todo");
    assert_eq!(todo.as_deref(), Some("milk"));
    let caption = sim.committed(FRIEND, group, captions(group), "beach");
    assert_eq!(caption.as_deref(), Some("sunset"));
    assert_eq!(folded(&sim, FRIEND, group), folded(&sim, PHONE, group));
}
