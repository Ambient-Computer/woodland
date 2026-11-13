mod batch;
mod hydration;
mod network;
mod outbox;
mod wire;

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

pub use batch::{Batch, TransactionBuilder};
use ed25519_dalek::VerifyingKey;
use hydration::Hydration;
use network::Network;
pub(crate) use outbox::Outbox;
use wire::Message;

use crate::actor::{self, Identity, Peer};
use crate::agreement::AgreementId;
use crate::encoding;
use crate::group::{Group, VerifiedParts};
use crate::io::{Action, Event, LocalEvent, NetworkEvent, StorageEvent};
use crate::storage::keys::{self, Record};

/// Represents this device. Applications should only create one.
///
/// Manages current state, updates, and subscriptions.
///
/// A state machine that never performs I/O itself. Each [`Batch`] of
/// events moves it to a new state, and the [`Action`]s it returns are how
/// that state reaches the outside world. Performing them eventually
/// produces new events (a reply arrives, a peer drops), which go into the
/// next batch.
///
/// Processing a batch has three stages:
///
/// 1. **Apply** each event in order. Domains take in changes and queue
///    storage work in the `Outbox`. Nothing is sent yet.
/// 2. **Advance** each group: learn, integrate, report progress, settle.
/// 3. **Compose** one message per connected peer, and order everything:
///    storage first, then scans, then sends.
#[derive(Debug)]
pub struct Node {
    /// Who this device is. Every local change is signed with it.
    identity: Identity,
    network: Network,
    /// Every agreement this device replicates.
    groups: BTreeMap<AgreementId, Group>,
    hydration: Hydration,
}

impl Node {
    /// Starts a node for this device, holding nothing yet. Call
    /// [`Batch::hydrate`] to load what's in storage.
    #[must_use]
    pub fn new(identity: Identity) -> Self {
        Self {
            identity,
            network: Network::default(),
            groups: BTreeMap::new(),
            hydration: Hydration::default(),
        }
    }

    /// Starts a batch of events to process together.
    pub fn batch(&mut self) -> Batch<'_> {
        Batch::new(self)
    }

    /// The contributors in the latest configuration this device has learned.
    #[must_use]
    pub fn contributors(&self, agreement: AgreementId) -> Vec<VerifyingKey> {
        let Some(group) = self.groups.get(&agreement) else {
            return Vec::new();
        };

        let configuration = &group.agreement().learned().configuration;
        configuration.members().iter().map(Peer::id).collect()
    }

    fn me(&self) -> Peer {
        self.identity.peer()
    }

    fn process(&mut self, events: Vec<Event>) -> Vec<Action> {
        let mut outbox = Outbox::default();
        for event in events {
            self.apply(event, &mut outbox);
        }

        for group in self.groups.values_mut() {
            group.advance(&self.identity, &mut outbox);
        }

        let messages = self.compose(&mut outbox);
        outbox.into_actions(messages)
    }

    fn apply(&mut self, event: Event, outbox: &mut Outbox) {
        match event {
            Event::Network(NetworkEvent::Connected { peer }) => {
                self.network.connect(peer);
            }

            Event::Network(NetworkEvent::Disconnected { peer }) => {
                self.network.disconnect(&peer);
                self.resubscribe();
            }

            Event::Network(NetworkEvent::Message { peer, bytes }) => {
                self.receive(peer, &bytes, outbox);
            }

            Event::Storage(StorageEvent::Hydrate) => hydration::start(outbox),

            Event::Storage(StorageEvent::Scanned { prefix, entries }) => {
                // A group scans its own data for snapshots. Every other scan
                // is part of hydration.
                let mut groups = self.groups.values_mut();
                if let Some(group) = groups.find(|it| keys::settled(it.id()) == prefix) {
                    group.scanned(&entries);
                    return;
                }

                let me = self.me();
                let restored =
                    hydration::scanned(&mut self.hydration, me, &prefix, &entries, outbox);
                if let Some(group) = restored {
                    self.groups.insert(group.id(), group);
                    self.resubscribe();
                }
            }

            Event::Storage(StorageEvent::Got { entries }) => {
                for (key, value) in entries {
                    self.got(&key, value.is_some());
                }
            }

            Event::Local(LocalEvent::Join { genesis }) => {
                let me = self.me();
                if let Entry::Vacant(vacant) = self.groups.entry(genesis.id()) {
                    vacant.insert(Group::join(&genesis, me, outbox));
                    self.resubscribe();
                }
            }

            Event::Local(LocalEvent::Reconfigure { agreement, change }) => {
                if let Some(group) = self.groups.get_mut(&agreement) {
                    group.propose(&self.identity, change);
                }
            }

            Event::Local(LocalEvent::Write {
                agreement,
                operations,
            }) => {
                if let Some(group) = self.groups.get_mut(&agreement) {
                    group.write(&self.identity, operations, outbox);
                }
            }
        }
    }

    /// A read is back. Groups read settled values to work out verdicts.
    fn got(&mut self, stored: &[u8], holds: bool) {
        if let Some(Record::Settled {
            agreement,
            model,
            key,
        }) = keys::parse(stored)
            && let Some(group) = self.groups.get_mut(&agreement)
        {
            group.got(model, key, holds);
        }
    }

    /// Decodes and validates a peer's message, then hands each agreement its
    /// part. Every signature in the message is checked in one batch first.
    fn receive(&mut self, from: Peer, bytes: &[u8], outbox: &mut Outbox) {
        let Some(mut message) = encoding::decode::<Message>(bytes) else {
            tracing::warn!(peer = ?from, "dropped an undecodable message");
            return;
        };

        if let Some(replicates) = message.hello.take() {
            self.network.heard_hello(from, replicates);
            self.resubscribe();
        }

        // One batch per payload type. Sharing a single batch would need one
        // signed type for both, which isn't worth it for one fewer call.
        let (mut attestations, mut transactions) = (Vec::new(), Vec::new());
        let (mut proposals, mut reports) = (Vec::new(), Vec::new());
        for part in message.groups.values_mut() {
            attestations.append(&mut part.attestations);
            transactions.append(&mut part.transactions);
            if let Some(gossip) = &mut part.gossip {
                proposals.append(&mut gossip.proposals);
                reports.append(&mut gossip.reports);
            }
        }

        let mut verified: BTreeMap<AgreementId, VerifiedParts> = BTreeMap::new();
        for attestation in actor::verify_all(attestations) {
            let agreement = attestation.payload().agreement;
            let part = verified.entry(agreement).or_default();
            part.attestations.push(attestation);
        }

        for proposal in actor::verify_all(proposals) {
            let agreement = proposal.payload().agreement;
            verified
                .entry(agreement)
                .or_default()
                .proposals
                .push(proposal);
        }

        for report in actor::verify_all(reports) {
            let agreement = report.payload().agreement;
            verified.entry(agreement).or_default().reports.push(report);
        }

        for transaction in actor::verify_all(transactions) {
            let agreement = transaction.payload().agreement;
            let part = verified.entry(agreement).or_default();
            part.transactions.push(transaction);
        }

        for (id, part) in message.groups {
            if let Some(group) = self.groups.get_mut(&id) {
                let signed = verified.remove(&id).unwrap_or_default();
                group.receive(from, &part, signed, outbox);
            }
        }
    }

    /// Keeps each group's peer list equal to the connected peers that
    /// replicate it.
    fn resubscribe(&mut self) {
        for (&id, group) in &mut self.groups {
            group.subscribe_only(&self.network.subscribers(id));
        }
    }

    /// One message per connected peer: a hello if our agreements changed,
    /// plus whatever each agreement says the peer is missing. Composing may
    /// ask storage for a snapshot, for next time.
    fn compose(&mut self, outbox: &mut Outbox) -> Vec<(Peer, Vec<u8>)> {
        let ours: BTreeSet<AgreementId> = self.groups.keys().copied().collect();
        let peers: Vec<Peer> = self.network.connected().collect();

        let mut messages = Vec::new();
        for peer in peers {
            let mut message = Message {
                hello: self.network.hello_for(peer, &ours),
                groups: BTreeMap::new(),
            };

            for (&id, group) in &mut self.groups {
                if let Some(part) = group.outgoing(peer, outbox) {
                    message.groups.insert(id, part);
                }
            }

            if message != Message::default() {
                messages.push((peer, encoding::encode(&message)));
            }
        }

        messages
    }
}

#[cfg(test)]
mod tests;
