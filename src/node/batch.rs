use ed25519_dalek::VerifyingKey;

use super::Node;
use crate::agreement::{AgreementId, Change, Genesis};
use crate::io::{Action, Event, LocalEvent, NetworkEvent, StorageEvent};
use crate::model::{ModelId, Operation};

/// Events to process together. Start one with [`Node::batch`].
///
/// Events are applied in the order they're added. Put everything that
/// happened since the last batch into one: the node bundles the results,
/// so peers get one message each and storage gets one atomic write.
#[derive(Debug)]
#[must_use = "nothing happens until the batch is processed"]
pub struct Batch<'a> {
    node: &'a mut Node,
    events: Vec<Event>,
}

/// A transaction being built: operations against one model, committed
/// atomically. Start one with [`Batch::transaction`].
#[derive(Debug)]
#[must_use = "the transaction is discarded unless committed"]
pub struct TransactionBuilder<'a> {
    batch: Batch<'a>,
    agreement: AgreementId,
    model: ModelId,
    operations: Vec<Operation>,
}

impl<'a> Batch<'a> {
    pub(super) fn new(node: &'a mut Node) -> Self {
        Self {
            node,
            events: Vec::new(),
        }
    }

    /// Loads what replication needs from storage: every agreement's
    /// protocol state, but no model data. Do this once, before reporting any
    /// connections. The node answers with scans. Report each result with
    /// [`Batch::scanned`].
    pub fn hydrate(self) -> Self {
        self.push(Event::Storage(StorageEvent::Hydrate))
    }

    /// A scan the node asked for finished.
    pub fn scanned(self, prefix: Vec<u8>, entries: Vec<(Vec<u8>, Vec<u8>)>) -> Self {
        self.push(Event::Storage(StorageEvent::Scanned { prefix, entries }))
    }

    /// A connection to `peer` opened.
    pub fn connected(self, peer: VerifyingKey) -> Self {
        let peer = peer.into();
        self.push(Event::Network(NetworkEvent::Connected { peer }))
    }

    /// The connection to `peer` closed.
    pub fn disconnected(self, peer: VerifyingKey) -> Self {
        let peer = peer.into();
        self.push(Event::Network(NetworkEvent::Disconnected { peer }))
    }

    /// `peer` sent a message.
    pub fn message(self, peer: VerifyingKey, bytes: Vec<u8>) -> Self {
        let peer = peer.into();
        self.push(Event::Network(NetworkEvent::Message { peer, bytes }))
    }

    /// Starts replicating an agreement. Founders and invited devices both
    /// join this way, with the same genesis.
    pub fn join(self, genesis: Genesis) -> Self {
        self.push(Event::Local(LocalEvent::Join { genesis }))
    }

    /// Proposes giving `contributor` write access.
    pub fn add_contributor(self, agreement: AgreementId, contributor: VerifyingKey) -> Self {
        let change = Change::Add(contributor.into());
        self.push(Event::Local(LocalEvent::Reconfigure { agreement, change }))
    }

    /// Proposes removing `contributor`. Removal is permanent for that key.
    /// Its past writes stay valid.
    pub fn remove_contributor(self, agreement: AgreementId, contributor: VerifyingKey) -> Self {
        let change = Change::Remove(contributor.into());
        self.push(Event::Local(LocalEvent::Reconfigure { agreement, change }))
    }

    /// Starts a transaction against `model`. Naming a new model ID creates
    /// it.
    pub fn transaction(self, agreement: AgreementId, model: ModelId) -> TransactionBuilder<'a> {
        TransactionBuilder {
            batch: self,
            agreement,
            model,
            operations: Vec::new(),
        }
    }

    /// Processes the events, returning a sequence of actions to perform.
    #[must_use = "actions must be performed"]
    pub fn process(self) -> Vec<Action> {
        self.node.process(self.events)
    }

    fn push(mut self, event: Event) -> Self {
        self.events.push(event);
        self
    }
}

impl<'a> TransactionBuilder<'a> {
    pub fn set(mut self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> Self {
        let (key, value) = (key.into(), value.into());
        self.operations.push(Operation::Set { key, value });
        self
    }

    pub fn remove(mut self, key: impl Into<Vec<u8>>) -> Self {
        let key = key.into();
        self.operations.push(Operation::Remove { key });
        self
    }

    /// Adds the transaction to the batch. Empty transactions are dropped.
    pub fn commit(self) -> Batch<'a> {
        if self.operations.is_empty() {
            return self.batch;
        }

        self.batch.push(Event::Local(LocalEvent::Write {
            agreement: self.agreement,
            model: self.model,
            operations: self.operations,
        }))
    }
}
