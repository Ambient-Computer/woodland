//! Joining a group that has a history already.

use super::*;

fn height(sim: &Sim, device: usize, group: GroupId) -> u64 {
    sim.devices[device].groups[&group].folded()
}

/// A group whose history spans a few checkpoints, kept whole.
fn seasoned() -> (Sim, GroupId) {
    let (mut sim, group) = Sim::group(4, 3);
    for round in 0..4 {
        sim.write_to(PHONE, group, set(notes(group), "round", &round.to_string()));
        sim.write_to(
            LAPTOP,
            group,
            claim(names(group), &round.to_string(), "laptop"),
        );
        sim.settle();
    }

    (sim, group)
}

#[test]
fn a_newcomer_starts_from_where_its_inviter_is() {
    let (mut sim, group) = seasoned();
    sim.rejoin(FRIEND, group, LAPTOP);
    assert_eq!(sim.committed(FRIEND, group, notes(group), "round"), None);

    // Nobody shed anything. It's handed the state all the same: it
    // can't commit its way there without every checkpoint in between.
    sim.connect(FRIEND, NAS);
    sim.run();

    assert_eq!(height(&sim, FRIEND, group), height(&sim, NAS, group));
    for model in [notes(group), names(group)] {
        let query = Query::prefix(group, model, b"").committed();
        assert_eq!(sim.read(FRIEND, &query), sim.read(NAS, &query));
    }

    // And follows along from there.
    sim.write_to(NAS, group, set(notes(group), "round", "later"));
    sim.connect_all();
    sim.settle();
    assert_eq!(
        sim.committed(FRIEND, group, notes(group), "round")
            .as_deref(),
        Some("later")
    );
}

#[test]
fn an_invitation_that_cant_be_read_is_refused() {
    let (mut sim, group) = seasoned();
    let mut invitation = sim.devices[LAPTOP].invite(group).expect("held");
    invitation.truncate(invitation.len() / 2);

    let joined = block_on(sim.devices[FRIEND].join(&invitation));

    assert!(matches!(joined, Err(Error::BadInvitation)));
}

/// The phone founded the group and was later removed. Its key can still
/// sign a first checkpoint, and a whole other history after it.
fn usurped() -> (Sim, GroupId) {
    let (mut sim, group) = seasoned();
    sim.reconfigure(LAPTOP, group, [LAPTOP, NAS]);
    sim.settle();

    sim.isolate(PHONE);
    let fresh = block_on(Database::open(MemoryStore::default(), secret(PHONE)));
    sim.devices[PHONE] = fresh.expect("opened");
    let (again, outputs) = block_on(sim.devices[PHONE].found([7; 32])).expect("founded");
    sim.absorb(PHONE, outputs);
    assert_eq!(again, group, "the same founding is the same group");
    sim.write_to(PHONE, group, set(notes(group), "round", "usurped"));
    sim.settle();
    assert_eq!(
        sim.committed(PHONE, group, notes(group), "round")
            .as_deref(),
        Some("usurped")
    );

    (sim, group)
}

#[test]
fn a_removed_founder_cant_rewrite_the_past_for_a_newcomer() {
    let (mut sim, group) = usurped();
    sim.rejoin(FRIEND, group, LAPTOP);

    sim.connect(FRIEND, PHONE);
    sim.settle();
    assert_eq!(sim.shown(FRIEND, group, notes(group), "round"), None);

    sim.connect(FRIEND, NAS);
    sim.settle();
    assert_eq!(
        sim.committed(FRIEND, group, notes(group), "round")
            .as_deref(),
        Some("3")
    );
}

/// An invitation is taken on trust. Whoever gives it decides which
/// history the newcomer believes.
#[test]
fn a_newcomer_believes_whoever_invited_it() {
    let (mut sim, group) = usurped();
    sim.rejoin(FRIEND, group, PHONE);

    sim.connect(FRIEND, PHONE);
    sim.connect(FRIEND, NAS);
    sim.settle();

    assert_eq!(
        sim.committed(FRIEND, group, notes(group), "round")
            .as_deref(),
        Some("usurped")
    );
}
