//! A member that says two things about one checkpoint.
//!
//! Like a fork, the honest way to do it is to lose a store: a device
//! that forgets what it voted for will vote again.

use super::super::message::{Message, Summary};
use super::super::proof::Proof;
use super::*;
use crate::actor::{Identity, Peer, Signed};
use crate::chain::{Ballot, Checkpoint, Evidence, Gossip, Vote};
use crate::encoding;

fn tip(sim: &Sim, device: usize, group: GroupId) -> Checkpoint {
    sim.devices[device].groups[&group].tip().clone()
}

fn has_a_say(sim: &Sim, device: usize, group: GroupId, member: usize) -> bool {
    let electorate = tip(sim, device, group).electorate;

    electorate.includes(Peer::from(sim.id(member)))
}

/// Two votes by `voter` in one attempt at the slot after `slot`.
fn two_votes(sim: &Sim, voter: usize, slot: &Checkpoint) -> Proof {
    let [one, other] = votes(sim, voter, slot);

    Proof::Conflict(Box::new(Evidence::Votes(one, other)))
}

fn votes(sim: &Sim, voter: usize, slot: &Checkpoint) -> [Signed<Vote>; 2] {
    let identity = Identity::from_secret_key(secret(voter));
    let ballot = Ballot {
        round: 1,
        proposer: Peer::from(sim.id(PHONE)),
    };
    let vote = |body: &[u8]| {
        let checkpoint = slot.extend(slot.electorate.clone(), body.to_vec());
        let vote = Vote {
            slot: slot.digest(),
            ballot,
            checkpoint: checkpoint.digest(),
        };

        Signed::sign(&identity, vote).signed().clone()
    };

    [vote(b"one"), vote(b"other")]
}

/// `to` is handed `proof` by `from`, with nothing else.
fn hand(sim: &mut Sim, from: usize, to: usize, group: GroupId, proof: Proof) {
    send(sim, from, to, group, Gossip::default(), vec![proof]);
}

fn send(sim: &mut Sim, from: usize, to: usize, group: GroupId, chain: Gossip, proofs: Vec<Proof>) {
    let message = Message {
        group,
        summary: Summary::default(),
        chain,
        entries: Vec::new(),
        restored: Vec::new(),
        proofs,
        snapshot: None,
    };
    let from = sim.id(from);
    let received = block_on(sim.devices[to].receive(from, &encoding::encode(&message)));
    sim.absorb(to, received.expect("received"));
}

#[test]
fn a_member_that_votes_twice_is_removed() {
    let (mut sim, group) = Sim::group(3, 3);
    let proof = two_votes(&sim, NAS, &tip(&sim, PHONE, group));

    hand(&mut sim, PHONE, LAPTOP, group, proof);
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert!(!has_a_say(&sim, device, group, NAS));
        assert!(has_a_say(&sim, device, group, PHONE));
    }

    let written = block_on(sim.devices[NAS].write(group, set(notes(group), "a", "b")));
    assert!(matches!(written, Err(Error::NotAContributor)));
}

/// Nobody has to put the proof together. Hearing both votes is enough.
#[test]
fn a_member_overheard_voting_twice_is_removed() {
    let (mut sim, group) = Sim::group(3, 3);
    let votes = votes(&sim, NAS, &tip(&sim, PHONE, group)).to_vec();
    let chain = Gossip {
        votes,
        ..Gossip::default()
    };

    send(&mut sim, PHONE, LAPTOP, group, chain, Vec::new());
    sim.settle();

    for device in [PHONE, LAPTOP, NAS] {
        assert!(!has_a_say(&sim, device, group, NAS));
    }
}

#[test]
fn the_proof_outlasts_a_restart() {
    let (mut sim, group) = Sim::group(3, 3);
    let proof = two_votes(&sim, NAS, &tip(&sim, PHONE, group));
    sim.disconnect_all();

    hand(&mut sim, PHONE, LAPTOP, group, proof);
    sim.restart(LAPTOP);
    sim.connect_all();
    sim.settle();

    assert!(!has_a_say(&sim, PHONE, group, NAS));
}

/// The same key may hold a role in another group. What it said there
/// is that group's business.
#[test]
fn what_was_said_about_another_chain_convicts_nobody_here() {
    let (mut sim, group) = Sim::group(3, 3);
    let elsewhere = Checkpoint::founding([9; 32], tip(&sim, PHONE, group).electorate);
    let proof = two_votes(&sim, NAS, &elsewhere);

    hand(&mut sim, PHONE, LAPTOP, group, proof);
    sim.settle();

    assert!(has_a_say(&sim, LAPTOP, group, NAS));
}

/// Votes mean nothing from someone with no say, so two of them prove
/// nothing worth acting on.
#[test]
fn votes_from_someone_with_no_say_convict_nobody() {
    let (mut sim, group) = Sim::group(4, 3);
    let roles = (0..4).map(|it| (it, Role::contributor(u32::from(it < 3))));
    sim.assign(PHONE, group, roles);
    sim.settle();
    let proof = two_votes(&sim, FRIEND, &tip(&sim, PHONE, group));

    hand(&mut sim, PHONE, LAPTOP, group, proof);
    sim.settle();

    sim.write_to(FRIEND, group, set(notes(group), "still", "writing"));
    sim.settle();
    assert_eq!(
        sim.committed(PHONE, group, notes(group), "still")
            .as_deref(),
        Some("writing")
    );
}
