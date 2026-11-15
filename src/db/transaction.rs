use crate::model::{Fingerprint, ModelId, Operation, Requirement};

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

    /// Drops a model and everything in it, for good.
    pub fn drop_model(mut self, model: ModelId) -> Self {
        self.operations.push(Operation::drop(model));
        self
    }
}
