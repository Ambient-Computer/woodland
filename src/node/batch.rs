use ed25519_dalek::VerifyingKey;

use super::Node;
use crate::agreement::{AgreementId, Change, Genesis};
use crate::io::{Action, Event, LocalEvent, NetworkEvent, StorageEvent};
use crate::model::{ColumnId, Fingerprint, ModelId, Operation, Requirement, Row, Rule, Value};

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

/// A transaction being built: operations against any of an agreement's
/// models, committed atomically. Start one with [`Batch::transaction`].
#[derive(Debug)]
#[must_use = "the transaction is discarded unless committed"]
pub struct TransactionBuilder<'a> {
    batch: Batch<'a>,
    agreement: AgreementId,
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

    /// A scan the node asked for finished. `start` and `end` are the
    /// ones it asked with.
    pub fn scanned(self, start: Vec<u8>, end: Vec<u8>, entries: Vec<(Vec<u8>, Vec<u8>)>) -> Self {
        let scanned = StorageEvent::Scanned {
            start,
            end,
            entries,
        };

        self.push(Event::Storage(scanned))
    }

    /// A get the node asked for finished. Report every key it asked for,
    /// with `None` for keys that aren't stored.
    pub fn got(self, entries: Vec<(Vec<u8>, Option<Vec<u8>>)>) -> Self {
        self.push(Event::Storage(StorageEvent::Got { entries }))
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

    /// Starts a transaction against the agreement's models.
    pub fn transaction(self, agreement: AgreementId) -> TransactionBuilder<'a> {
        TransactionBuilder {
            batch: self,
            agreement,
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
    /// Sets `key` in `model`, a [key-value] or [threshold] model. Naming a
    /// new model ID creates it.
    ///
    /// [key-value]: ModelId::kv
    /// [threshold]: ModelId::threshold
    pub fn set(
        mut self,
        model: ModelId,
        key: impl Into<Vec<u8>>,
        value: impl Into<Vec<u8>>,
    ) -> Self {
        let operation = Operation::set(model, key.into(), value.into());
        self.operations.push(operation);
        self
    }

    /// Claims `key` in `model`, a [claims] model, for `value`. If the key
    /// is already claimed at this transaction's place in the timeline,
    /// the whole transaction is dropped, on every device. Read the key to
    /// see who has it.
    ///
    /// [claims]: ModelId::claims
    pub fn claim(
        mut self,
        model: ModelId,
        key: impl Into<Vec<u8>>,
        value: impl Into<Vec<u8>>,
    ) -> Self {
        let operation = Operation::claim(model, key.into(), value.into());
        self.operations.push(operation);
        self
    }

    /// Gives up `key` in `model`, a [claims] model, so that it can be
    /// claimed again. Only what it was claimed for can: the whole
    /// transaction is dropped, on every device, unless at its place in
    /// the timeline the key is claimed for exactly `value`.
    ///
    /// [claims]: ModelId::claims
    pub fn release(
        mut self,
        model: ModelId,
        key: impl Into<Vec<u8>>,
        value: impl Into<Vec<u8>>,
    ) -> Self {
        let operation = Operation::release(model, key.into(), value.into());
        self.operations.push(operation);
        self
    }

    /// Makes the transaction depend on what its author read. The whole
    /// transaction is dropped, on every device, unless at its place in
    /// the timeline `key` in `model` holds exactly `value`. Nothing is
    /// written.
    ///
    /// Two devices that read the same value, and each write a new one
    /// expecting the old, can't both happen: whichever comes second in
    /// the timeline finds the first one's value instead.
    ///
    /// It's also how to hold a lock: [claim] a key for a value nobody
    /// else would use, and expect it in every transaction that needs
    /// the lock. If another device turns out to have claimed it first,
    /// those transactions are dropped along with the claim.
    ///
    /// [claim]: Self::claim
    pub fn expect(
        mut self,
        model: ModelId,
        key: impl Into<Vec<u8>>,
        value: impl AsRef<[u8]>,
    ) -> Self {
        let requirement = Requirement::Is(Fingerprint::of(value.as_ref()));
        let operation = Operation::expect(model, key.into(), requirement);
        self.operations.push(operation);
        self
    }

    /// Like [`expect`], for a key that should hold nothing.
    ///
    /// [`expect`]: Self::expect
    pub fn expect_absent(mut self, model: ModelId, key: impl Into<Vec<u8>>) -> Self {
        let operation = Operation::expect(model, key.into(), Requirement::Vacant);
        self.operations.push(operation);
        self
    }

    /// Like [`expect`], for a row of `table` as it was read. The whole
    /// transaction is dropped, on every device, if at its place in the
    /// timeline the row has been written to or deleted since.
    ///
    /// [`expect`]: Self::expect
    pub fn expect_unchanged(mut self, table: ModelId, row: &Row) -> Self {
        let (key, requirement) = row.unchanged();
        self.operations
            .push(Operation::expect(table, key, requirement));
        self
    }

    /// Creates `table`, a [table] model, with these columns and their
    /// names. If it already exists at this transaction's place in the
    /// timeline, the whole transaction is dropped, on every device.
    ///
    /// [table]: ModelId::table
    pub fn create_table(
        mut self,
        table: ModelId,
        columns: impl IntoIterator<Item = (ColumnId, impl Into<String>)>,
    ) -> Self {
        let columns = columns.into_iter().map(|(id, name)| (id, name.into()));
        let operation = Operation::create_table(table, columns.collect());
        self.operations.push(operation);
        self
    }

    /// Adds a row to `table` under `key`, with a value for each of
    /// these columns. The whole transaction is dropped, on every
    /// device, unless at its place in the timeline the table and each
    /// of the columns exist, and no row has the key.
    ///
    /// The table may have been created by a transaction that hasn't
    /// settled, this one included. If that one ends up dropped, so does
    /// this.
    pub fn insert(
        self,
        table: ModelId,
        key: impl Into<Vec<u8>>,
        cells: impl IntoIterator<Item = (ColumnId, Value)>,
    ) -> Self {
        self.put_row(table, key, cells, Rule::Insert)
    }

    /// Replaces the row of `table` under `key` with these cells. A
    /// column that isn't given a value is left without one. The whole
    /// transaction is dropped, on every device, unless at its place in
    /// the timeline the table, the columns and the row exist.
    pub fn replace(
        self,
        table: ModelId,
        key: impl Into<Vec<u8>>,
        cells: impl IntoIterator<Item = (ColumnId, Value)>,
    ) -> Self {
        self.put_row(table, key, cells, Rule::Replace)
    }

    /// Writes the row of `table` under `key`, whether or not there is
    /// one already. The table and the columns still have to exist.
    pub fn upsert(
        self,
        table: ModelId,
        key: impl Into<Vec<u8>>,
        cells: impl IntoIterator<Item = (ColumnId, Value)>,
    ) -> Self {
        self.put_row(table, key, cells, Rule::Upsert)
    }

    /// Removes the row of `table` under `key`. Removing a row that
    /// isn't there does nothing, but the table has to exist.
    pub fn delete(mut self, table: ModelId, key: impl Into<Vec<u8>>) -> Self {
        self.operations
            .push(Operation::delete_row(table, key.into()));
        self
    }

    /// Adds `column` to `table` under `name`. The whole transaction is
    /// dropped, on every device, if at its place in the timeline the
    /// table doesn't exist or the column was ever added before. A
    /// column that was dropped can't come back: make a new one.
    pub fn add_column(mut self, table: ModelId, column: ColumnId, name: impl Into<String>) -> Self {
        let operation = Operation::name_column(table, column, name.into(), true);
        self.operations.push(operation);
        self
    }

    /// Changes the name of `column`, which the table has to have.
    /// Rows aren't touched: they know the column by its ID.
    pub fn rename_column(
        mut self,
        table: ModelId,
        column: ColumnId,
        name: impl Into<String>,
    ) -> Self {
        let operation = Operation::name_column(table, column, name.into(), false);
        self.operations.push(operation);
        self
    }

    /// Removes `column` from `table`. Transactions that write to the
    /// column later in the timeline are dropped whole, on every device.
    /// What rows already held in it is no longer read.
    pub fn drop_column(mut self, table: ModelId, column: ColumnId) -> Self {
        self.operations.push(Operation::drop_column(table, column));
        self
    }

    fn put_row(
        mut self,
        table: ModelId,
        key: impl Into<Vec<u8>>,
        cells: impl IntoIterator<Item = (ColumnId, Value)>,
        rule: Rule,
    ) -> Self {
        let cells = cells.into_iter().collect();
        self.operations
            .push(Operation::put_row(table, key.into(), cells, rule));
        self
    }

    /// Removes `key` from `model`.
    pub fn remove(mut self, model: ModelId, key: impl Into<Vec<u8>>) -> Self {
        self.operations.push(Operation::remove(model, key.into()));
        self
    }

    /// Drops `model`: everything in it is gone, and it's dead for good.
    /// Transactions that name it later in the timeline are dropped whole,
    /// on every device.
    pub fn drop_model(mut self, model: ModelId) -> Self {
        self.operations.push(Operation::drop(model));
        self
    }

    /// Adds the transaction to the batch. Empty transactions are dropped.
    pub fn commit(self) -> Batch<'a> {
        if self.operations.is_empty() {
            return self.batch;
        }

        self.batch.push(Event::Local(LocalEvent::Write {
            agreement: self.agreement,
            operations: self.operations,
        }))
    }
}
