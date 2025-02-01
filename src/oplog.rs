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
pub enum Commit {
    /// The first commit of the operation log. Represents a fresh history.
    Initial,

    /// An entry in the operation log.
    Entry {
        parent: Box<Commit>,
        operations: Vec<Datatype>,
    },
}

pub struct OperationLog {
    events: HashSet<Datatype>,
    commits: HashSet<Commit>,
    head: Box<Commit>,
}

impl OperationLog {
    pub fn new() -> Self {
        Self {
            events: HashSet::new(),
            commits: HashSet::from([Commit::Initial]),
            head: Box::new(Commit::Initial),
        }
    }

    pub fn commit(&mut self, operations: Vec<Datatype>) -> Commit {
        self.events.extend(operations.clone());

        let commit = Commit::Entry {
            parent: self.head.clone(),
            operations: operations.clone(),
        };

        self.commits.insert(commit.clone());
        self.head = Box::new(commit.clone());

        commit
    }

    fn linearize(&self) -> Vec<Datatype> {
        let mut linearized = Vec::new();
        let mut current = self.head.clone();

        loop {
            match current.as_ref() {
                Commit::Initial => break,
                Commit::Entry { parent, operations } => {
                    linearized.extend(operations.clone());
                    current = parent.clone();
                }
            }
        }

        linearized
    }

    pub fn derive(&self, counter: &mut crate::counter::Counter) {
        for operation in self.linearize() {
            let Datatype::Counter(operation) = operation;
            counter.update(operation);
        }
    }
}
