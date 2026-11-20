//! A sealed group: read by its contributors and readers, and carried
//! by anyone.

use std::collections::BTreeMap;

use super::*;
use crate::actor::Identity;
use crate::chain::Electorate;
use crate::db::group::Sealing;
use crate::seal::Secret;

#[cfg(feature = "blobs")]
mod blobs;
mod rotation;
mod snapshots;

const GUEST: usize = 4;

/// Long and unlikely enough to look for in what a device holds.
const NEEDLE: &str = "the surprise party is on saturday";

fn key(sim: &Sim, device: usize) -> SealingKey {
    sim.devices[device].sealing_key()
}

fn roles(sim: &Sim) -> [(usize, Role); 4] {
    [
        (PHONE, Role::contributor(1).sealed_to(key(sim, PHONE))),
        (LAPTOP, Role::contributor(1).sealed_to(key(sim, LAPTOP))),
        (NAS, Role::witness(1)),
        (FRIEND, Role::reader(key(sim, FRIEND))),
    ]
}

/// The phone founds a sealed group. It and the laptop write, the NAS
/// is a witness, the friend reads, and the guest has no role. Nobody
/// is connected.
fn founded() -> (Sim, GroupId) {
    let mut sim = Sim::new(5);
    let (group, outputs) = block_on(sim.devices[PHONE].found_sealed([7; 32])).expect("founded");
    sim.absorb(PHONE, outputs);
    let invitation = sim.devices[PHONE].invite(group).expect("held");
    for device in 1..5 {
        let (_, outputs) = block_on(sim.devices[device].join(&invitation)).expect("joined");
        sim.absorb(device, outputs);
    }
    sim.home = group;

    (sim, group)
}

fn sealed() -> (Sim, GroupId) {
    let (mut sim, group) = founded();
    sim.connect_all();
    let roles = roles(&sim);
    sim.assign(PHONE, group, roles);
    sim.settle();

    (sim, group)
}

/// Whether `needle` is anywhere in what the device stores.
fn stores(sim: &Sim, device: usize, group: GroupId, needle: &str) -> bool {
    let (start, end) = keys::range(keys::group(group));
    let view = sim.devices[device].store.view();
    let stored = block_on(crate::db::step::scan(&view, &start, &end)).expect("scanned");
    let found = |bytes: &Vec<u8>| {
        bytes
            .windows(needle.len())
            .any(|it| it == needle.as_bytes())
    };

    stored.iter().any(|(key, value)| found(key) || found(value))
}

/// Delivers everything, and returns which devices were sent `needle`.
fn sent(sim: &mut Sim, needle: &str) -> BTreeSet<usize> {
    let mut told = BTreeSet::new();
    while let Some((_, to, message)) = sim.queue.front() {
        if message
            .windows(needle.len())
            .any(|it| it == needle.as_bytes())
        {
            told.insert(*to);
        }
        sim.deliver();
    }

    told
}

fn read_note(sim: &Sim, device: usize, group: GroupId, key: &str) -> Option<String> {
    sim.committed(device, group, notes(group), key)
}

#[test]
fn contributors_and_readers_read_and_everyone_else_carries() {
    let (mut sim, group) = sealed();

    sim.write_to(PHONE, group, set(notes(group), "plan", NEEDLE));
    let told = sent(&mut sim, NEEDLE);
    sim.settle();

    assert_eq!(told, BTreeSet::new(), "it never travels in the clear");
    for device in [PHONE, LAPTOP, FRIEND] {
        assert!(sim.devices[device].reads(group), "device {device}");
        let note = read_note(&sim, device, group, "plan");
        assert_eq!(note.as_deref(), Some(NEEDLE), "device {device}");
    }
    for device in [NAS, GUEST] {
        assert!(!sim.devices[device].reads(group), "device {device}");
        assert_eq!(read_note(&sim, device, group, "plan"), None);
        assert_eq!(sim.shown(device, group, notes(group), "plan"), None);
        assert!(!stores(&sim, device, group, NEEDLE), "device {device}");
        assert_eq!(sim.entries_kept(device, group), 1, "it holds the entry");
    }
}

#[test]
fn a_device_that_cant_read_passes_on_what_it_holds() {
    let (mut sim, group) = sealed();
    sim.disconnect_all();

    // The phone and the laptop never meet. The guest sees each in
    // turn, and what it carries between them is enough to commit.
    sim.write_to(PHONE, group, set(notes(group), "plan", NEEDLE));
    sim.tick(PHONE);
    for _ in 0..3 {
        for device in [PHONE, LAPTOP] {
            sim.disconnect_all();
            sim.connect(GUEST, device);
            sim.run();
        }
    }

    for device in [PHONE, LAPTOP] {
        let note = read_note(&sim, device, group, "plan");
        assert_eq!(note.as_deref(), Some(NEEDLE), "device {device}");
    }
    assert!(!stores(&sim, GUEST, group, NEEDLE));
}

#[test]
fn a_witness_helps_commit_what_it_cant_read() {
    let (mut sim, group) = sealed();
    sim.disconnect_all();
    sim.connect(PHONE, NAS);

    sim.write_to(PHONE, group, set(notes(group), "plan", NEEDLE));
    sim.settle();

    assert_eq!(
        read_note(&sim, PHONE, group, "plan").as_deref(),
        Some(NEEDLE)
    );
    assert!(!stores(&sim, NAS, group, NEEDLE));
}

#[test]
fn everyone_who_reads_a_sealed_group_is_named_with_a_key() {
    let (mut sim, group) = sealed();
    let before = sim.devices[PHONE].groups[&group].tip().height;

    let mut keyless = roles(&sim).to_vec();
    keyless.push((GUEST, Role::contributor(0)));
    let keyless: Vec<_> = keyless
        .into_iter()
        .map(|(device, role)| (sim.id(device), role))
        .collect();
    let asked = block_on(sim.devices[PHONE].reconfigure(group, keyless));

    assert!(matches!(asked, Err(Error::NotAContributor)));
    sim.settle();
    assert_eq!(sim.devices[PHONE].groups[&group].tip().height, before);
}

#[test]
fn a_device_given_a_key_reads_what_it_was_only_carrying() {
    let (mut sim, group) = sealed();
    sim.write_to(PHONE, group, set(notes(group), "plan", NEEDLE));
    sim.settle();
    sim.write_to(LAPTOP, group, set(notes(group), "cake", "lemon"));
    sim.run();
    assert_eq!(read_note(&sim, GUEST, group, "plan"), None);

    let mut roles = roles(&sim).to_vec();
    roles.push((GUEST, Role::reader(key(&sim, GUEST))));
    sim.assign(PHONE, group, roles);
    sim.settle();

    assert!(sim.devices[GUEST].reads(group));
    assert_eq!(
        read_note(&sim, GUEST, group, "plan").as_deref(),
        Some(NEEDLE)
    );
    let cake = read_note(&sim, GUEST, group, "cake");
    assert_eq!(cake.as_deref(), Some("lemon"));
    assert!(sim.changes[GUEST].contains(&Changed::Everything { group }));
}

#[test]
fn a_sealed_group_survives_a_restart() {
    let (mut sim, group) = sealed();
    sim.write_to(PHONE, group, set(notes(group), "plan", NEEDLE));
    sim.settle();

    for device in [PHONE, FRIEND, GUEST] {
        sim.restart(device);
    }
    sim.connect_all();
    sim.write_to(PHONE, group, set(notes(group), "cake", "lemon"));
    sim.settle();

    for device in [PHONE, FRIEND] {
        assert_eq!(
            read_note(&sim, device, group, "plan").as_deref(),
            Some(NEEDLE)
        );
        let cake = read_note(&sim, device, group, "cake");
        assert_eq!(cake.as_deref(), Some("lemon"), "device {device}");
    }
    assert!(!sim.devices[GUEST].reads(group));
    assert_eq!(read_note(&sim, GUEST, group, "cake"), None);
}

#[test]
fn an_open_group_stays_open() {
    let (mut sim, group) = Sim::group(4, 3);
    sim.write_to(PHONE, group, set(notes(group), "plan", NEEDLE));
    sim.settle();

    for device in [PHONE, FRIEND] {
        assert!(sim.devices[device].reads(group));
        assert!(stores(&sim, device, group, NEEDLE));
    }
}

/// The roles of [`sealed`], with the guest as a reader too.
fn with_guest(sim: &Sim) -> Vec<(usize, Role)> {
    let mut roles = roles(sim).to_vec();
    roles.push((GUEST, Role::reader(key(sim, GUEST))));

    roles
}

fn electorate(sim: &Sim, roles: Vec<(usize, Role)>) -> Electorate {
    let peer = |device: usize| sim.devices[device].identity.peer();

    Electorate::of(roles.into_iter().map(|(device, role)| (peer(device), role)))
}

/// What the laptop and the guest each make of a checkpoint the phone
/// proposes to give `roles`, once it's been tampered with.
fn judged_of(roles: Vec<(usize, Role)>, tamper: impl FnOnce(&mut Option<Sealing>)) -> [bool; 2] {
    let (sim, group) = sealed();
    let electorate = electorate(&sim, roles);
    let phone = &sim.devices[PHONE];
    let candidate = phone.groups[&group].candidate(&phone.identity, electorate, tamper);

    [LAPTOP, GUEST].map(|device| {
        let device = &sim.devices[device];

        device.groups[&group].sealed_soundly(&device.identity, &candidate)
    })
}

/// What the NAS, which holds no secret, makes of the same.
fn witnessed(tamper: impl FnOnce(&mut Option<Sealing>)) -> bool {
    let (sim, group) = sealed();
    let electorate = electorate(&sim, with_guest(&sim));
    let phone = &sim.devices[PHONE];
    let candidate = phone.groups[&group].candidate(&phone.identity, electorate, tamper);
    let nas = &sim.devices[NAS];

    nas.groups[&group].sealed_soundly(&nas.identity, &candidate)
}

/// The same, for a checkpoint that makes the guest a reader.
fn judged(tamper: impl FnOnce(&mut Option<Sealing>)) -> [bool; 2] {
    judged_of(with_guest(&Sim::new(5)), tamper)
}

fn sealing(said: &mut Option<Sealing>) -> &mut Sealing {
    said.as_mut().expect("a sealed group")
}

fn another_secret() -> Secret {
    Secret::make(&Identity::from_secret_key([9; 32]), b"another")
}

#[test]
fn a_checkpoint_hands_the_secret_to_whoever_it_newly_names() {
    let guest = Sim::new(5).devices[GUEST].identity.peer();

    assert_eq!(judged(|_| {}), [true, true]);
    assert_eq!(judged(|said| *said = None), [false, false]);
    assert_eq!(
        judged(|said| drop(sealing(said).grants.remove(&guest))),
        [false, false]
    );
    assert_eq!(judged(|said| sealing(said).epoch += 1), [false, false]);
    assert_eq!(
        judged(|said| sealing(said).commitment = another_secret().commitment()),
        [false, false]
    );
}

/// What states are named under never changes, and whoever holds the
/// secret can tell whether a checkpoint gives it to them.
#[test]
fn a_checkpoint_hands_on_what_states_are_named_under() {
    let renamed = |said: &mut Option<Sealing>| {
        sealing(said).blinded = another_secret().commitment();
    };
    let withheld = |said: &mut Option<Sealing>| sealing(said).blind = vec![7; 80];

    assert_eq!(judged(renamed), [false, false]);
    assert!(witnessed(|_| {}));
    assert!(!witnessed(renamed));
    assert_eq!(judged(withheld), [false, false]);
}

/// Only the one it's for can tell a copy of the secret is no good.
/// The laptop can't, and the guest was granted the secret all the same
/// by being named.
#[test]
fn a_device_handed_the_wrong_secret_doesnt_vouch_for_it() {
    let sim = Sim::new(5);
    let (guest, key) = (sim.devices[GUEST].identity.peer(), key(&sim, GUEST));
    let forger = Identity::from_secret_key([9; 32]);

    let wrong = another_secret().grant(&forger, &key, b"something else");
    let judged = judged(|said| drop(sealing(said).grants.insert(guest, wrong)));

    assert_eq!(judged, [true, false]);
}

#[test]
fn an_open_group_says_nothing_of_secrets() {
    let (sim, group) = Sim::group(4, 3);
    let phone = &sim.devices[PHONE];
    let held = &phone.groups[&group];
    let electorate = held.tip().electorate.clone();
    let sealing = Sealing {
        epoch: 0,
        commitment: another_secret().commitment(),
        grants: BTreeMap::new(),
        previous: None,
        blinded: another_secret().commitment(),
        blind: Vec::new(),
    };

    let plain = held.candidate(&phone.identity, electorate.clone(), |_| {});
    let sealed = held.candidate(&phone.identity, electorate, |said| *said = Some(sealing));

    assert!(held.sealed_soundly(&phone.identity, &plain));
    assert!(!held.sealed_soundly(&phone.identity, &sealed));
}

#[test]
fn nobody_vouches_for_someone_who_reads_with_no_key() {
    let mut keyless = roles(&Sim::new(5)).to_vec();
    keyless.push((GUEST, Role::contributor(0)));

    assert_eq!(judged_of(keyless, |_| {}), [false, false]);
}

/// The phone, up to no good, tries for a checkpoint that makes the
/// guest a reader.
fn put_forward(sim: &mut Sim, group: GroupId, tamper: impl FnOnce(&mut Option<Sealing>)) {
    let electorate = electorate(sim, with_guest(sim));
    let phone = &mut sim.devices[PHONE];
    let held = phone.groups.get_mut(&group).expect("held");
    held.put_forward(&phone.identity, electorate, tamper);
    sim.settle();
}

#[test]
fn a_checkpoint_that_keeps_the_secret_from_someone_it_names_isnt_voted_for() {
    let (mut sim, group) = sealed();
    let guest = sim.devices[GUEST].identity.peer();
    let before = sim.devices[LAPTOP].groups[&group].tip().height;

    put_forward(&mut sim, group, |said| {
        sealing(said).grants.remove(&guest);
    });

    for device in [PHONE, LAPTOP, NAS, GUEST] {
        let tip = sim.devices[device].groups[&group].tip();
        assert_eq!(tip.height, before, "device {device}");
    }
}

/// Nobody but the guest can tell its copy is of some other secret, so
/// the checkpoint is decided. The guest doesn't take the copy for the
/// real thing. It was named, so it's due the secret, and whoever has
/// it hands it over.
#[test]
fn a_peer_whose_copy_was_no_good_is_handed_the_secret_by_another() {
    let (mut sim, group) = sealed();
    sim.write_to(PHONE, group, set(notes(group), "plan", NEEDLE));
    sim.settle();
    let guest = sim.devices[GUEST].identity.peer();
    let about = crate::db::group::sealed_about(group, 0);
    let wrong = another_secret().grant(&sim.devices[PHONE].identity, &key(&sim, GUEST), &about);

    put_forward(&mut sim, group, |said| {
        sealing(said).grants.insert(guest, wrong);
    });
    let told = sent(&mut sim, NEEDLE);
    sim.settle();

    assert_eq!(told, BTreeSet::new());
    assert!(
        sim.devices[GUEST].groups[&group]
            .tip()
            .electorate
            .is_reader(guest)
    );
    assert!(sim.devices[GUEST].reads(group));
    assert_eq!(
        read_note(&sim, GUEST, group, "plan").as_deref(),
        Some(NEEDLE)
    );
}

/// Where each device stands follows the chain: a role or none, and
/// for one that lost a role, whether it ever read.
#[test]
fn a_device_says_where_it_stands() {
    use crate::Standing;

    let (mut sim, group) = sealed();
    let standing = |sim: &Sim, device: usize| sim.devices[device].standing(group);
    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        assert_eq!(standing(&sim, device), Some(Standing::Member), "{device}");
    }
    assert_eq!(standing(&sim, GUEST), Some(Standing::Outside));
    let stranger = GroupId::from_bytes([9; 32]);
    assert_eq!(sim.devices[PHONE].standing(stranger), None);

    // The friend and the NAS are removed. One of them read.
    let fewer = roles(&sim).into_iter().take(2);
    sim.assign(PHONE, group, fewer);
    sim.settle();
    assert_eq!(standing(&sim, FRIEND), Some(Standing::ShutOut));
    assert_eq!(standing(&sim, NAS), Some(Standing::Outside));

    // Let back in, the friend is a member again once it hears.
    sim.isolate(FRIEND);
    let all = roles(&sim);
    sim.assign(PHONE, group, all);
    sim.settle();
    assert_eq!(standing(&sim, FRIEND), Some(Standing::ShutOut));
    sim.connect_all();
    sim.settle();
    assert_eq!(standing(&sim, FRIEND), Some(Standing::Member));
}

/// Whoever holds a role in any group this device holds, and whoever
/// it has asked for, once each and without itself.
#[test]
fn a_device_lists_who_is_worth_a_connection() {
    let (mut sim, group) = sealed();
    let ids = |sim: &Sim, devices: &[usize]| -> BTreeSet<[u8; 32]> {
        devices.iter().map(|&it| sim.id(it).to_bytes()).collect()
    };
    let listed = |sim: &Sim, device: usize| -> BTreeSet<[u8; 32]> {
        let members = sim.devices[device].members();

        members.iter().map(VerifyingKey::to_bytes).collect()
    };
    assert_eq!(listed(&sim, PHONE), ids(&sim, &[LAPTOP, NAS, FRIEND]));
    assert_eq!(
        listed(&sim, GUEST),
        ids(&sim, &[PHONE, LAPTOP, NAS, FRIEND])
    );

    sim.isolate(PHONE);
    let more = roles(&sim)
        .into_iter()
        .chain([(GUEST, Role::reader(key(&sim, GUEST)))]);
    sim.assign(PHONE, group, more);
    assert_eq!(
        listed(&sim, PHONE),
        ids(&sim, &[LAPTOP, NAS, FRIEND, GUEST])
    );
    assert_eq!(sim.devices[PHONE].members().len(), 4);
}

#[test]
fn a_founder_reads_what_it_writes_before_anyone_joins() {
    let (mut sim, group) = founded();

    sim.write_to(PHONE, group, set(notes(group), "plan", NEEDLE));
    sim.settle();
    sim.restart(PHONE);

    assert_eq!(
        read_note(&sim, PHONE, group, "plan").as_deref(),
        Some(NEEDLE)
    );
}
