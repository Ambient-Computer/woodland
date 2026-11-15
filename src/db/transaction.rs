use crate::model::{ColumnId, Fingerprint, ModelId, Operation, Requirement, Row, Value};

/// A set of changes that happen together, or not at all.
///
/// It says what its author did and what that depended on, never how to
/// do it again. Nothing here is worked out by anyone else.
#[derive(Clone, Debug, Default)]
#[must_use = "a transaction does nothing until it's committed"]
pub struct Transaction {
    pub(crate) operations: Vec<Operation>,
}

impl Transaction {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `key` in a key-value or threshold model. Naming a new model
    /// creates it.
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

    /// Removes `key` from a key-value or threshold model.
    pub fn remove(mut self, model: ModelId, key: impl Into<Vec<u8>>) -> Self {
        self.operations.push(Operation::remove(model, key.into()));
        self
    }

    /// Takes `key` in a claims model, if nobody has. Of two transactions
    /// that try, the first in the timeline happens.
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

    /// Gives back a claim that holds `value`.
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

    /// Makes the whole transaction depend on `key` still holding
    /// `value`, as its author read it.
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

    /// Makes the whole transaction depend on `key` holding nothing.
    pub fn expect_absent(mut self, model: ModelId, key: impl Into<Vec<u8>>) -> Self {
        let operation = Operation::expect(model, key.into(), Requirement::Vacant);
        self.operations.push(operation);
        self
    }

    /// Like [`expect`], for a row of `table` as it was read. The whole
    /// transaction is dropped, on every device, if at its place in the
    /// timeline the row has been deleted, or any of its columns has
    /// been written to since.
    ///
    /// [`expect`]: Self::expect
    pub fn expect_unchanged(mut self, table: ModelId, row: &Row) -> Self {
        let expect = |(key, requirement)| Operation::expect(table, key, requirement);
        self.operations
            .extend(row.unchanged().into_iter().map(expect));
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
    /// of the columns exist, no row has the key, and no row has any of
    /// its values in a [unique] column.
    ///
    /// The table may have been created by a transaction that hasn't
    /// been committed, this one included. If that one ends up dropped, so does
    /// this.
    ///
    /// [unique]: ColumnId::unique
    pub fn insert(
        mut self,
        table: ModelId,
        key: impl Into<Vec<u8>>,
        cells: impl IntoIterator<Item = (ColumnId, Value)>,
    ) -> Self {
        let cells = cells.into_iter().collect();
        self.operations
            .push(Operation::insert_row(table, key.into(), cells));
        self
    }

    /// Changes `row` of `table`: each of these columns gets its new
    /// value, and the rest are left alone. Another device that changes
    /// other columns of the row at the same time changes those, and
    /// both happen. If both change the same column, the later in the
    /// timeline is what's read.
    ///
    /// The whole transaction is dropped, on every device, if at its
    /// place in the timeline the row has been deleted, or a column
    /// given a value is gone.
    ///
    /// A [unique] or [indexed] column is stricter. The transaction is
    /// also dropped if any of the row's unique or indexed columns have
    /// changed since it was read, or another row has the new value of
    /// a unique one.
    ///
    /// To change the row only if nothing at all has changed, add
    /// [`expect_unchanged`].
    ///
    /// [unique]: ColumnId::unique
    /// [indexed]: ColumnId::indexed
    /// [`expect_unchanged`]: Self::expect_unchanged
    pub fn update(
        mut self,
        table: ModelId,
        row: &Row,
        changes: impl IntoIterator<Item = (ColumnId, Value)>,
    ) -> Self {
        self.operations
            .push(Operation::table(table, row.set(changes)));
        self
    }

    /// Removes `row` of `table`. The whole transaction is dropped, on
    /// every device, if at its place in the timeline the row is
    /// already gone, or its [unique] or [indexed] columns have changed
    /// since it was read.
    ///
    /// It's also dropped if anything has ever [referred] to the row,
    /// unless the row was [retired] when it was read and still is, in
    /// the same retirement. See [`retire`] for how to delete a row
    /// that others depend on.
    ///
    /// [unique]: ColumnId::unique
    /// [indexed]: ColumnId::indexed
    /// [referred]: Self::refer
    /// [retired]: Row::is_retired
    /// [`retire`]: Self::retire
    pub fn delete_row(mut self, table: ModelId, row: &Row) -> Self {
        self.operations.push(Operation::table(table, row.delete()));
        self
    }

    /// Makes the transaction depend on `row` of `table`, the way a row
    /// with a foreign key depends on the row it names. The whole
    /// transaction is dropped, on every device, if at its place in the
    /// timeline the row is gone or [retired].
    ///
    /// The row is marked as referred to, for good, and can't be
    /// deleted without being retired first. So of a reference and a
    /// delete written apart, only one happens. Any number of
    /// references to a row can be written apart, and all happen.
    ///
    /// [retired]: Self::retire
    pub fn refer(mut self, table: ModelId, row: &Row) -> Self {
        self.operations.push(Operation::table(table, row.refer()));
        self
    }

    /// Retires `row` of `table`: from here on in the timeline, nothing
    /// can [refer] to it. The whole transaction is dropped, on every
    /// device, if the row is gone or already retired.
    ///
    /// It's the first step of deleting a row that's been referred to:
    ///
    /// 1. Retire it.
    /// 2. Wait until a [committed] read shows it retired. Every
    ///    reference to it that will ever exist is committed by then.
    /// 3. Find what refers to it, with committed reads.
    /// 4. In one transaction, deal with those rows and
    ///    [delete][`delete_row`] this one. Or [`restore`] it.
    ///
    /// [refer]: Self::refer
    /// [committed]: crate::Query::committed
    /// [`delete_row`]: Self::delete_row
    /// [`restore`]: Self::restore
    pub fn retire(mut self, table: ModelId, row: &Row) -> Self {
        self.operations.push(Operation::table(table, row.retire()));
        self
    }

    /// Lets a [retired] row take references again. A delete written
    /// while it was retired doesn't happen after this.
    ///
    /// [retired]: Self::retire
    pub fn restore(mut self, table: ModelId, row: &Row) -> Self {
        self.operations.push(Operation::table(table, row.restore()));
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

    /// Drops a model and everything in it, for good.
    pub fn drop_model(mut self, model: ModelId) -> Self {
        self.operations.push(Operation::drop(model));
        self
    }
}
