//! Simulates contributors running consensus over an unreliable network.
//!
//! Each node runs the real roles and signs real messages. A schedule
//! decides what happens next: which message arrives, which is lost or
//! duplicated, who proposes. There's no clock, only steps, so any
//! interleaving a network could produce is a schedule away.
//!
//! Invariants are checked after every step, not just at the end, so a
//! failure points at the step that broke them.

mod agreement;
mod liveness;
mod restarts;
mod retrying;

use ed25519_dalek::Signature;
use proptest::sample::Index;

use crate::envelope::{Envelope, Payload};
use crate::quorum::consensus::{
    Accept, Accepted, Acceptor, Ballot, Certificate, Phase, Prepare, Promise, Proposer, Refusal,
    Rejected, Tally,
};
use crate::quorum::{ContributorChange, Operation};
use crate::testing::{Actor, Ledger, PublicKey, keys, laptop, nas, phone, quorum};

type Signed<T> = Envelope<PublicKey, Signature, T>;

#[derive(Clone, Debug)]
enum Message {
    Prepare(Signed<Prepare<PublicKey>>),
    Promise(Signed<Promise<PublicKey>>),
    Accept(Signed<Accept<PublicKey>>),
    Accepted(Signed<Accepted<PublicKey>>),
    Rejected(Signed<Rejected<PublicKey>>),

    /// Proof of a commit, for a node that missed it. Needs no envelope:
    /// it carries its voters' signatures.
    Certificate(Certificate<PublicKey, Signature>),
}

/// A message on its way to one node.
#[derive(Clone, Debug)]
struct Packet {
    to: usize,
    message: Message,
}

/// What can happen next.
#[derive(Clone, Debug)]
enum Event {
    /// A node starts proposing its operation.
    Propose(usize),

    /// A message in flight arrives.
    Deliver(Index),

    /// A message in flight is lost.
    Drop(Index),

    /// A message in flight arrives twice, e.g. through two peers.
    Duplicate(Index),

    /// A node restarts, keeping only what its roles say to persist.
    Restart(usize),

    /// A node resends whatever it's still waiting on (its proposer's
    /// request, and its acceptor's vote), and shares its latest commit.
    Advertise(usize),
}

#[derive(Debug)]
struct Node {
    actor: Actor,
    ledger: Ledger,
    acceptor: Acceptor<PublicKey>,
    tally: Tally<PublicKey, Signature>,
    proposer: Option<Proposer<PublicKey>>,

    /// Gone for good, e.g. a lost phone. Hears nothing, says nothing.
    gone: bool,

    /// What this node would propose. Each node wants something different,
    /// so agreement means something.
    operation: Operation<PublicKey>,
}

#[derive(Debug)]
struct Network {
    nodes: Vec<Node>,
    in_flight: Vec<Packet>,

    /// Every commit any node made, as (node, certificate).
    commits: Vec<(usize, Certificate<PublicKey, Signature>)>,

    /// Each node's acceptor as of the last check, as (epoch, promised).
    promised: Vec<(u64, Option<Ballot<PublicKey>>)>,
}

impl Network {
    /// `phone`, `laptop`, and `nas` in one quorum. Each wants to remove
    /// the next one, so every proposal is valid and no two are the same.
    fn new() -> Self {
        let actors = [phone(), laptop(), nas()];
        let ledger = quorum(&[&actors[0], &actors[1], &actors[2]]);
        let removes = [nas().key(), phone().key(), laptop().key()];

        let nodes = actors
            .into_iter()
            .zip(removes)
            .map(|(actor, removed)| Node {
                actor,
                ledger: ledger.clone(),
                acceptor: Acceptor::new(0),
                tally: Tally::new(0),
                proposer: None,
                gone: false,
                operation: Operation::Contributors(ContributorChange {
                    added: keys(&[]),
                    removed: [removed].into(),
                }),
            })
            .collect();

        Self {
            promised: vec![(0, None); 3],
            nodes,
            in_flight: Vec::new(),
            commits: Vec::new(),
        }
    }

    fn run(&mut self, schedule: &[Event]) {
        for event in schedule {
            self.step(event);
            self.check();
        }
    }

    /// Delivers everything still in flight, in order, until the network
    /// goes quiet.
    ///
    /// Dueling proposers could keep it busy forever, so it gives up
    /// eventually.
    fn settle(&mut self) {
        for _ in 0..10_000 {
            if self.in_flight.is_empty() {
                return;
            }

            let packet = self.in_flight.remove(0);
            self.receive(packet);
            self.check();
        }

        panic!("dueling proposers never settled");
    }

    /// Lets the network recover: no more losses, and every node keeps
    /// advertising until nothing changes.
    fn heal(&mut self) {
        loop {
            self.settle();
            let before = self.commits.len();
            for node in 0..self.nodes.len() {
                self.advertise(node);
            }

            self.settle();
            if self.commits.len() == before {
                return;
            }
        }
    }

    fn step(&mut self, event: &Event) {
        match event {
            Event::Propose(node) => self.propose(*node),
            Event::Deliver(index) => {
                if let Some(index) = self.pick(*index) {
                    let packet = self.in_flight.swap_remove(index);
                    self.receive(packet);
                }
            }
            Event::Drop(index) => {
                if let Some(index) = self.pick(*index) {
                    self.in_flight.swap_remove(index);
                }
            }
            Event::Duplicate(index) => {
                if let Some(index) = self.pick(*index) {
                    self.in_flight.push(self.in_flight[index].clone());
                }
            }
            Event::Restart(node) => self.restart(*node),
            Event::Advertise(node) => self.advertise(*node),
        }
    }

    fn pick(&self, index: Index) -> Option<usize> {
        (!self.in_flight.is_empty()).then(|| index.index(self.in_flight.len()))
    }

    /// Starts a proposal, unless the node already started one this epoch
    /// or its proposal no longer fits.
    fn propose(&mut self, at: usize) {
        let node = &mut self.nodes[at];
        if node.proposer.is_some() || node.ledger.epoch() > 0 {
            return;
        }

        let Ok(proposer) = Proposer::new(&node.ledger, node.actor.key(), node.operation.clone())
        else {
            return;
        };

        let prepare = sign(node, proposer.prepare());
        node.proposer = Some(proposer);
        self.broadcast(&Message::Prepare(prepare));
    }

    /// Forgets everything documented as not persisted.
    fn restart(&mut self, at: usize) {
        let node = &mut self.nodes[at];
        node.tally = Tally::new(node.ledger.epoch());

        if let Some(Proposer {
            phase: Phase::Preparing { promises },
            ..
        }) = &mut node.proposer
        {
            promises.clear();
        }
    }

    fn advertise(&mut self, at: usize) {
        let node = &self.nodes[at];
        if node.gone {
            return;
        }

        let mut messages = Vec::new();

        if let Some(proposer) = &node.proposer {
            if let Some(accept) = proposer.accept() {
                messages.push(Message::Accept(sign(node, accept)));
            } else {
                messages.push(Message::Prepare(sign(node, proposer.prepare())));
            }
        }

        if let Some(proposal) = &node.acceptor.accepted {
            let accepted = Accepted {
                epoch: node.acceptor.epoch,
                proposal: proposal.clone(),
            };

            messages.push(Message::Accepted(sign(node, accepted)));
        }

        // Without it, a node that missed every vote never catches up. If
        // it has nothing of its own to advertise, nobody even hears of it.
        if let Some(certificate) = node.ledger.commits().last() {
            messages.push(Message::Certificate(certificate.clone()));
        }

        for message in &messages {
            self.broadcast(message);
        }
    }

    fn vanish(&mut self, at: usize) {
        self.nodes[at].gone = true;
        self.in_flight.retain(|packet| packet.to != at);
    }

    fn receive(&mut self, packet: Packet) {
        let at = packet.to;
        let node = &mut self.nodes[at];
        if node.gone {
            return;
        }

        match packet.message {
            Message::Prepare(prepare) => match node.acceptor.prepare(&node.ledger, &prepare) {
                Ok(promise) => {
                    let to = self.index_of(&promise.ballot.proposer);
                    let promise = sign(&self.nodes[at], promise);
                    self.send(to, Message::Promise(promise));
                }
                Err(refusal) => self.refused(at, refusal),
            },

            Message::Promise(promise) => {
                let Some(proposer) = &mut node.proposer else {
                    return;
                };

                let accept = proposer
                    .promise(&node.ledger, &promise)
                    .expect("honest nodes send valid promises");

                if let Some(accept) = accept {
                    let accept = sign(node, accept);
                    self.broadcast(&Message::Accept(accept));
                }
            }

            Message::Accept(accept) => match node.acceptor.accept(&node.ledger, &accept) {
                Ok(accepted) => {
                    let accepted = sign(node, accepted);
                    self.broadcast(&Message::Accepted(accepted));
                }
                Err(refusal) => self.refused(at, refusal),
            },

            Message::Accepted(accepted) => {
                let certificate = node
                    .tally
                    .count(&node.ledger, accepted)
                    .expect("honest nodes send valid votes");

                if let Some(certificate) = certificate {
                    self.commit(at, certificate);
                }
            }

            Message::Rejected(rejected) => {
                let Some(proposer) = &mut node.proposer else {
                    return;
                };

                let retry = proposer
                    .outbid(&node.ledger, &rejected)
                    .expect("honest nodes send valid rejections");

                if let Some(retry) = retry {
                    let retry = sign(node, retry);
                    self.broadcast(&Message::Prepare(retry));
                }
            }

            Message::Certificate(certificate) => {
                if certificate.epoch == node.ledger.epoch() {
                    self.commit(at, certificate);
                }
            }
        }
    }

    /// Answers a refusal the way an application would. Only `Superseded`
    /// has an answer; the rest are expected when messages arrive late.
    fn refused(&mut self, at: usize, refusal: Refusal<PublicKey>) {
        match refusal {
            Refusal::Superseded(rejected) => {
                let to = self.index_of(&rejected.ballot.proposer);
                let rejected = sign(&self.nodes[at], rejected);
                self.send(to, Message::Rejected(rejected));
            }
            Refusal::EpochOver => {}
            other => panic!("unexpected refusal from an honest network: {other}"),
        }
    }

    /// Appends a certificate, then starts the node's roles over for the
    /// next epoch.
    fn commit(&mut self, at: usize, certificate: Certificate<PublicKey, Signature>) {
        let node = &mut self.nodes[at];
        certificate
            .verify(&node.ledger)
            .expect("honest nodes build valid certificates");

        node.ledger.append(certificate.clone());
        node.acceptor = Acceptor::new(node.ledger.epoch());
        node.tally = Tally::new(node.ledger.epoch());
        node.proposer = None;

        self.commits.push((at, certificate));
    }

    /// Checks what must hold after every step, whatever the schedule.
    fn check(&mut self) {
        // Agreement: nobody commits a different operation for an epoch
        // someone else already committed.
        for (_, certificate) in &self.commits {
            let first = self
                .commits
                .iter()
                .find(|(_, other)| other.epoch == certificate.epoch)
                .map(|(_, first)| &first.proposal.operation);

            assert_eq!(
                Some(&certificate.proposal.operation),
                first,
                "two operations committed for epoch {}",
                certificate.epoch
            );
        }

        // Validity: only operations someone proposed get committed.
        for (_, certificate) in &self.commits {
            assert!(
                self.nodes
                    .iter()
                    .any(|node| node.operation == certificate.proposal.operation),
                "committed an operation nobody proposed"
            );
        }

        // Promises only ever rise within an epoch. An acceptor that took
        // one back could let a lower ballot through.
        for (node, last) in self.nodes.iter().zip(&mut self.promised) {
            let now = (node.acceptor.epoch, node.acceptor.promised.clone());
            assert!(now >= *last, "an acceptor's promise went backwards");
            *last = now;
        }
    }

    /// Delivers the first message in flight to `to` that `matches`, for
    /// tests that need one exact interleaving.
    fn deliver_first(&mut self, to: usize, matches: impl Fn(&Peek) -> bool) {
        let id = self.nodes[to].ledger.id();
        let index = self
            .in_flight
            .iter()
            .position(|packet| packet.to == to && matches(&Peek::of(&packet.message, &id)))
            .expect("a matching message is in flight");

        let packet = self.in_flight.remove(index);
        self.receive(packet);
        self.check();
    }

    fn broadcast(&mut self, message: &Message) {
        for to in 0..self.nodes.len() {
            self.send(to, message.clone());
        }
    }

    fn send(&mut self, to: usize, message: Message) {
        self.in_flight.push(Packet { to, message });
    }

    fn index_of(&self, key: &PublicKey) -> usize {
        self.nodes
            .iter()
            .position(|node| &node.actor.key() == key)
            .expect("ballots belong to nodes in the network")
    }
}

/// What a test needs to tell messages apart.
#[derive(Debug)]
struct Peek {
    kind: Kind,
    ballot: Ballot<PublicKey>,
    operation: Option<Operation<PublicKey>>,
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Prepare,
    Promise,
    Accept,
    Accepted,
    Rejected,
    Certificate,
}

impl Peek {
    fn of(message: &Message, quorum: &blake3::Hash) -> Self {
        let peek =
            |kind, ballot: &Ballot<PublicKey>, operation: Option<&Operation<PublicKey>>| Self {
                kind,
                ballot: ballot.clone(),
                operation: operation.cloned(),
            };

        match message {
            Message::Prepare(m) => {
                let m = m.verify(quorum).expect("honest messages verify");
                peek(Kind::Prepare, &m.ballot, None)
            }
            Message::Promise(m) => {
                let m = m.verify(quorum).expect("honest messages verify");
                peek(Kind::Promise, &m.ballot, None)
            }
            Message::Accept(m) => {
                let m = m.verify(quorum).expect("honest messages verify");
                peek(
                    Kind::Accept,
                    &m.proposal.ballot,
                    Some(&m.proposal.operation),
                )
            }
            Message::Accepted(m) => {
                let m = m.verify(quorum).expect("honest messages verify");
                peek(
                    Kind::Accepted,
                    &m.proposal.ballot,
                    Some(&m.proposal.operation),
                )
            }
            Message::Rejected(m) => {
                let m = m.verify(quorum).expect("honest messages verify");
                peek(Kind::Rejected, &m.ballot, None)
            }
            Message::Certificate(m) => peek(
                Kind::Certificate,
                &m.proposal.ballot,
                Some(&m.proposal.operation),
            ),
        }
    }
}

fn sign<T: Payload>(node: &Node, payload: T) -> Signed<T> {
    Envelope::sign(&node.actor, &node.ledger.id(), payload).expect("test keys always sign")
}

// The harness itself: without a lossy schedule, one proposal commits
// everywhere. If this fails, the simulation tests prove nothing.
#[test]
fn commits_when_nothing_goes_wrong() {
    let mut network = Network::new();
    network.propose(0);
    network.settle();

    assert_eq!(network.commits.len(), network.nodes.len());
    for node in &network.nodes {
        assert_eq!(node.ledger.epoch(), 1);
    }
}
