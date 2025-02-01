use std::hash::Hash;

use crate::oplog::Datatype;

type SIZE = u32;

#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub enum Operation {
    Increment(SIZE),
    Decrement(SIZE),
    Set(SIZE),
}

pub struct Counter;

impl Counter {
    pub fn new() -> Self {
        Self
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
}
