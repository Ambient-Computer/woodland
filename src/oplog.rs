use std::collections::HashSet;
use std::hash::Hash;

/// Groupings of operations by the datatype they modify.
#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub enum Datatype {
    /// Operate on counters, the "hello world" of CRDTs.
    Counter(crate::counter::Operation),
}

/// Commit operations. Create new commit objects that form links in the operation log.
#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub enum Operation {
    /// The first commit of the operation log. Represents a fresh history.
    Init,

    /// An entry in the operation log.
    Commit {
        parent: Box<Operation>,
        operations: Vec<Datatype>,
    },
}

pub struct OperationLog {
    events: HashSet<Datatype>,
    commits: HashSet<Operation>,
    head: Box<Operation>,
}

impl OperationLog {
    pub fn new() -> Self {
        Self {
            events: HashSet::new(),
            commits: HashSet::from([Operation::Init]),
            head: Box::new(Operation::Init),
        }
    }

    pub fn commit(&mut self, operations: Vec<Datatype>) -> Operation {
        self.events.extend(operations.clone());

        let commit = Operation::Commit {
            parent: self.head.clone(),
            operations: operations.clone(),
        };

        self.commits.insert(commit.clone());

        commit
    }
}
