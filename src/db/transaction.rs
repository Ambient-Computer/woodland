use crate::db::ClockId;
use crate::model::table::{Deferred, Each};
use ed25519_dalek::VerifyingKey;

use crate::actor::Peer;
use crate::model::{
    Allowance, ColumnId, Filter, Fingerprint, ModelId, Operation, Requirement, Row, Value,
};

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

    /// The clock whose models this writes to, or `fallback` if it
    /// writes nothing. `None` if it writes to more than one.
    pub(crate) fn clock(&self, fallback: ClockId) -> Option<ClockId> {
        let mut clocks = self.operations.iter().map(|it| it.model.clock());
        let first = clocks.next().unwrap_or(fallback);

        clocks.all(|clock| clock == first).then_some(first)
    }

    /// Adds an operation.
    pub(crate) fn with(mut self, operation: Operation) -> Self {
        self.operations.push(operation);
        self
    }

    /// How many operations it has.
    pub(crate) fn len(&self) -> usize {
        self.operations.len()
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

    /// Adds a row, like [`insert`], and gives it the next number of
    /// `column`'s sequence once the transaction is committed: one more
    /// than the last row numbered there, starting from 1. No number is
    /// ever skipped or given twice, because none is given out until
    /// nothing can change which row came first.
    ///
    /// Until then the row is there, and `column` holds nothing. That
    /// can take as long as the group's next checkpoint.
    ///
    /// `column` must hold whole numbers, or the transaction is dropped.
    ///
    /// [`insert`]: Self::insert
    pub fn insert_numbered(
        self,
        table: ModelId,
        key: impl Into<Vec<u8>>,
        cells: impl IntoIterator<Item = (ColumnId, Value)>,
        column: ColumnId,
    ) -> Self {
        let row: Vec<u8> = key.into();
        let mut numbered = self.insert(table, row.clone(), cells);
        let number = Deferred::Number {
            row,
            incarnation: None,
            column,
        };
        numbered.operations.push(Operation::deferred(table, number));
        numbered
    }

    /// Gives `row` of `table` the next number of `column`'s sequence,
    /// once the transaction is committed. See [`insert_numbered`].
    ///
    /// The whole transaction is dropped, for good, if by then the row
    /// is gone or has a number in `column` already.
    ///
    /// [`insert_numbered`]: Self::insert_numbered
    pub fn number(mut self, table: ModelId, row: &Row, column: ColumnId) -> Self {
        let number = row.number(column);
        self.operations.push(Operation::deferred(table, number));
        self
    }

    /// Changes every row of `table` that `filter` picks, of the rows
    /// there are once the transaction is committed: each gets these
    /// values in these columns. Nothing changes until then, which can
    /// take as long as the group's next checkpoint.
    ///
    /// Use it when the rows can't be known in advance. Rows that are
    /// known are better [updated] one by one, which doesn't wait.
    ///
    /// The whole transaction is dropped, for good, if any of those
    /// rows can't take the change: a column is gone, or two rows would
    /// hold one value in a [unique] column.
    ///
    /// [updated]: Self::update
    /// [unique]: ColumnId::unique
    pub fn update_where(
        mut self,
        table: ModelId,
        filter: Filter,
        changes: impl IntoIterator<Item = (ColumnId, Value)>,
    ) -> Self {
        let change = Each::Set(changes.into_iter().collect());
        let each = Deferred::Each { filter, change };
        self.operations.push(Operation::deferred(table, each));
        self
    }

    /// Removes every row of `table` that `filter` picks, of the rows
    /// there are once the transaction is committed. Like
    /// [`update_where`], nothing changes until then.
    ///
    /// The whole transaction is dropped, for good, if anything has
    /// [referred] to one of those rows and the row isn't [retired].
    ///
    /// [`update_where`]: Self::update_where
    /// [referred]: Self::refer
    /// [retired]: Self::retire
    pub fn delete_where(mut self, table: ModelId, filter: Filter) -> Self {
        let change = Each::Delete;
        let each = Deferred::Each { filter, change };
        self.operations.push(Operation::deferred(table, each));
        self
    }

    /// Makes a rule of `filter`: once the transaction is committed,
    /// every row of `table` has to be one it picks. A transaction that
    /// would leave a row it doesn't pick is dropped, for good, when
    /// its turn comes to be committed. Until then such a row can be
    /// read, like anything else that isn't final.
    ///
    /// The whole transaction is dropped, for good, if a row there
    /// already breaks the rule. Change those rows first, in an earlier
    /// transaction or with [`update_where`] earlier in this one.
    ///
    /// A column with no value fails every test of order, so a rule
    /// that a number is [at least] something also says it's there.
    ///
    /// [`update_where`]: Self::update_where
    /// [at least]: Filter::at_least
    pub fn add_rule(mut self, table: ModelId, filter: Filter) -> Self {
        let rule = Deferred::Rule { filter };
        self.operations.push(Operation::deferred(table, rule));
        self
    }

    /// Stops enforcing a rule made of exactly this `filter`. Unlike
    /// making one, it doesn't wait.
    ///
    /// The whole transaction is dropped if, at its place in the
    /// timeline, there's no such rule.
    pub fn lift_rule(mut self, table: ModelId, filter: &Filter) -> Self {
        self.operations
            .push(Operation::table(table, Row::lift(filter)));
        self
    }

    /// Removes `row` of `table` along with every row that refers to
    /// it, once the transaction is committed. Nothing changes until
    /// then, which can take as long as the group's next checkpoint.
    ///
    /// `referrers` says where to look: each is a table in the same
    /// clock and a column of it, holding the key of the row its own
    /// row [refers] to, as text or bytes. Every row that names `row`
    /// in one of them is deleted with it. The row doesn't have to be
    /// [retired] first: once this is committed, no reference can land
    /// before it, and any placed after it doesn't happen.
    ///
    /// The whole transaction is dropped, for good, if by then the row
    /// is gone, or something has referred in turn to a row that would
    /// go with it.
    ///
    /// [refers]: Self::refer
    /// [retired]: Self::retire
    pub fn delete_with(
        mut self,
        table: ModelId,
        row: &Row,
        referrers: impl IntoIterator<Item = (ModelId, ColumnId)>,
    ) -> Self {
        let remove = row.remove(referrers, true);
        self.operations.push(Operation::deferred(table, remove));
        self
    }

    /// Removes `row` of `table` once the transaction is committed, if
    /// by then no row names it in any of `referrers`: see
    /// [`delete_with`]. If one does, the whole transaction is dropped,
    /// for good.
    ///
    /// Unlike [`delete_row`], it doesn't mind that the row was
    /// referred to once, by rows that are gone.
    ///
    /// [`delete_with`]: Self::delete_with
    /// [`delete_row`]: Self::delete_row
    pub fn delete_unreferred(
        mut self,
        table: ModelId,
        row: &Row,
        referrers: impl IntoIterator<Item = (ModelId, ColumnId)>,
    ) -> Self {
        let remove = row.remove(referrers, false);
        self.operations.push(Operation::deferred(table, remove));
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

    /// Gives `table` a name, in place of any it had. The table has to
    /// exist. Nothing else about it changes: everything is kept under
    /// its ID, and whoever knows it by an older name still writes to
    /// the same table.
    pub fn rename_table(mut self, table: ModelId, name: impl Into<String>) -> Self {
        let name = name.into();
        let operation = Operation::table(table, crate::model::table::Operation::Name { name });
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

    /// Removes `column` from `table`, once the transaction is
    /// committed. Until then nothing changes, which can take as long
    /// as the group's next checkpoint.
    ///
    /// Transactions that write to the column later in the timeline
    /// are dropped whole, on every device, when their turn comes to
    /// be committed. What rows already held in it is no longer read.
    pub fn drop_column(mut self, table: ModelId, column: ColumnId) -> Self {
        self.operations.push(Operation::drop_column(table, column));
        self
    }

    /// Drops a model and everything in it, for good, once the
    /// transaction is committed. Until then nothing changes, and it
    /// shows as [pending].
    ///
    /// Transactions that name the model later in the timeline are
    /// dropped whole, on every device, when their turn comes to be
    /// committed.
    ///
    /// [pending]: crate::Database::pending
    pub fn drop_model(mut self, model: ModelId) -> Self {
        self.operations.push(Operation::drop(model));
        self
    }

    /// Starts `quantity` in an [allowances] model: there are `total`
    /// of it, all in `owner`'s allowance, which must be this device's.
    /// Of two transactions that open one quantity, the first in the
    /// timeline happens.
    ///
    /// [allowances]: ModelId::allowances
    pub fn open_allowance(
        mut self,
        model: ModelId,
        quantity: impl Into<Vec<u8>>,
        total: u64,
        owner: VerifyingKey,
    ) -> Self {
        let open = Allowance::open(quantity.into(), total, Peer::from(owner));
        self.operations.push(Operation::allowance(model, open));
        self
    }

    /// Uses up `amount` of an allowance, which must be this device's,
    /// as it was just read. Nothing another device does stops it.
    ///
    /// The whole transaction is dropped, on every device, if `amount`
    /// is more than the allowance, or if the allowance isn't what was
    /// read at the transaction's place in the timeline: something this
    /// device wrote before it didn't happen.
    pub fn spend(mut self, model: ModelId, allowance: &Allowance, amount: u64) -> Self {
        let spend = allowance.spend(amount);
        self.operations.push(Operation::allowance(model, spend));
        self
    }

    /// Moves `amount` out of an allowance, which must be this
    /// device's, for `to` to [accept]. Until it does, nobody can spend
    /// it. Dropped like a [spend] that asks for too much.
    ///
    /// [accept]: Self::accept
    /// [spend]: Self::spend
    pub fn give(
        mut self,
        model: ModelId,
        allowance: &Allowance,
        to: VerifyingKey,
        amount: u64,
    ) -> Self {
        let give = allowance.give(Peer::from(to), amount);
        self.operations.push(Operation::allowance(model, give));
        self
    }

    /// Takes everything an allowance has been [given] into it. The
    /// allowance must be this device's, as it was just read.
    ///
    /// The whole transaction is dropped if, at its place in the
    /// timeline, one of those gifts isn't there: giving it didn't
    /// happen.
    ///
    /// [given]: Self::give
    pub fn accept(mut self, model: ModelId, allowance: &Allowance) -> Self {
        let accepted = allowance.accept().into_iter();
        self.operations
            .extend(accepted.map(|it| Operation::allowance(model, it)));
        self
    }
}
