use super::super::{Ballot, Chain, Checkpoint, Electorate, Gossip};
use super::*;
use crate::testing::peers::{identity_of, laptop, nas, phone};

fn founding() -> Checkpoint {
    Checkpoint::founding([7; 32], Electorate::new([(phone(), 1), (laptop(), 1)]))
}

fn ballot(round: u64) -> Ballot {
    Ballot {
        round,
        proposer: phone(),
    }
}

fn candidate(body: &[u8]) -> Checkpoint {
    founding().extend(founding().electorate, body.to_vec())
}

fn vote(by: Peer, round: u64, body: &[u8]) -> Signed<Vote> {
    let vote = Vote {
        slot: founding().digest(),
        ballot: ballot(round),
        checkpoint: candidate(body).digest(),
    };

    Signed::sign(&identity_of(by), vote).signed().clone()
}

fn promise(by: Peer, round: u64, accepted: Option<(u64, &[u8])>) -> Signed<Promise> {
    let promise = Promise {
        slot: founding().digest(),
        ballot: ballot(round),
        accepted: accepted.map(|(round, body)| (ballot(round), candidate(body))),
    };

    Signed::sign(&identity_of(by), promise).signed().clone()
}

fn proposal(round: u64, body: &[u8]) -> Signed<Proposal> {
    let proposal = Proposal {
        slot: founding().digest(),
        ballot: ballot(round),
        checkpoint: candidate(body),
    };

    Signed::sign(&identity_of(phone()), proposal)
        .signed()
        .clone()
}

fn convicts(evidence: &Evidence) -> Option<Peer> {
    evidence.check().map(|conflict| conflict.author)
}

#[test]
fn two_votes_in_one_attempt_convict() {
    let evidence = Evidence::Votes(vote(laptop(), 1, b"a"), vote(laptop(), 1, b"b"));

    assert_eq!(convicts(&evidence), Some(laptop()));
}

#[test]
fn votes_an_honest_signer_could_cast_dont() {
    let again = Evidence::Votes(vote(laptop(), 1, b"a"), vote(laptop(), 1, b"a"));
    let later = Evidence::Votes(vote(laptop(), 1, b"a"), vote(laptop(), 2, b"b"));
    let others = Evidence::Votes(vote(laptop(), 1, b"a"), vote(phone(), 1, b"b"));

    assert_eq!(convicts(&again), None);
    assert_eq!(convicts(&later), None);
    assert_eq!(convicts(&others), None);
}

#[test]
fn two_checkpoints_named_for_one_attempt_convict() {
    let evidence = Evidence::Proposals(proposal(1, b"a"), proposal(1, b"b"));
    let honest = Evidence::Proposals(proposal(1, b"a"), proposal(2, b"b"));

    assert_eq!(convicts(&evidence), Some(phone()));
    assert_eq!(convicts(&honest), None);
}

#[test]
fn a_vote_the_promise_ruled_out_convicts() {
    let unreported = Evidence::Promise(promise(laptop(), 3, None), vote(laptop(), 2, b"a"));
    let after = Evidence::Promise(
        promise(laptop(), 3, Some((1, b"a"))),
        vote(laptop(), 2, b"b"),
    );
    let another = Evidence::Promise(
        promise(laptop(), 3, Some((1, b"a"))),
        vote(laptop(), 1, b"b"),
    );

    assert_eq!(convicts(&unreported), Some(laptop()));
    assert_eq!(convicts(&after), Some(laptop()));
    assert_eq!(convicts(&another), Some(laptop()));
}

#[test]
fn a_vote_the_promise_allows_doesnt() {
    let reported = Evidence::Promise(
        promise(laptop(), 3, Some((2, b"a"))),
        vote(laptop(), 2, b"a"),
    );
    let earlier = Evidence::Promise(
        promise(laptop(), 3, Some((2, b"a"))),
        vote(laptop(), 1, b"b"),
    );
    let kept = Evidence::Promise(promise(laptop(), 3, None), vote(laptop(), 3, b"a"));
    let beyond = Evidence::Promise(promise(laptop(), 3, None), vote(laptop(), 4, b"a"));
    let others = Evidence::Promise(promise(phone(), 3, None), vote(laptop(), 2, b"a"));

    for honest in [reported, earlier, kept, beyond, others] {
        assert_eq!(convicts(&honest), None);
    }
}

#[test]
fn a_chain_notices_when_it_hears_both() {
    let mut chain = Chain::new(founding());
    let heard = |votes| Gossip {
        votes,
        ..Gossip::default()
    };

    chain.receive(heard(vec![vote(laptop(), 1, b"a")]));
    assert_eq!(chain.evidence(), vec![]);

    chain.receive(heard(vec![vote(laptop(), 1, b"b")]));
    let found = chain.evidence();
    assert_eq!(found.len(), 1);
    assert_eq!(convicts(&found[0]), Some(laptop()));
    assert_eq!(chain.evidence(), vec![], "handed over once");
}

#[test]
fn a_chain_notices_a_broken_promise() {
    let mut chain = Chain::new(founding());
    chain.receive(Gossip {
        promises: vec![promise(laptop(), 3, None)],
        ..Gossip::default()
    });
    chain.receive(Gossip {
        votes: vec![vote(laptop(), 2, b"a")],
        ..Gossip::default()
    });

    let found = chain.evidence();

    assert_eq!(found.len(), 1);
    assert!(matches!(found[0], Evidence::Promise(..)));
}

#[test]
fn a_conflict_concerns_only_those_whose_word_counted() {
    let chain = Chain::new(founding());
    let conflict = |author, voter| Conflict {
        author,
        slot: founding().digest(),
        voter,
    };
    let elsewhere = Conflict {
        slot: candidate(b"a").digest(),
        ..conflict(laptop(), true)
    };

    assert_eq!(chain.concerns(&conflict(laptop(), true)), Some(true));
    assert_eq!(chain.concerns(&conflict(nas(), true)), Some(false));
    assert_eq!(chain.concerns(&conflict(nas(), false)), Some(false));
    assert_eq!(chain.concerns(&elsewhere), None);
}
