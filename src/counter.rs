use std::hash::Hash;

use crate::oplog::Datatype;

type SIZE = u32;

#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub enum Operation {
    Increment(SIZE),
    Decrement(SIZE),
    Set(SIZE),
}

pub struct Counter(pub SIZE);

impl Counter {
    pub fn new() -> Self {
        Self(0)
    }

    pub fn increment(&self, count: SIZE) -> Datatype {
        Datatype::Counter(Operation::Increment(count))
    }

    pub fn decrement(&self, count: SIZE) -> Datatype {
        Datatype::Counter(Operation::Decrement(count))
    }

    pub fn set(&self, value: SIZE) -> Datatype {
        Datatype::Counter(Operation::Set(value))
    }

    pub fn update(&mut self, operation: Operation) {
        self.0 = match operation {
            Operation::Increment(count) => self.0 + count,
            Operation::Decrement(count) => self.0 - count,
            Operation::Set(value) => value,
        }
    }
}
