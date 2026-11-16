//! Entries: what a contributor appends to its chain.
//!
//! An entry is a signed header and a payload. The header names the entry
//! before it by hash, so a contributor's entries form a chain that can't
//! be rewritten, and it names the heads of everyone else's chains that
//! its author had taken in, which is what puts entries from different
//! authors in causal order.
//!
//! Names are hashes, never bare counters. A counter can't tell two
//! entries at one position apart, and a contributor that signs two is
//! exactly what has to be noticed.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::actor::{Identity, Peer, Signable, Signed, verify_all};
use crate::chain::Digest;
use crate::clock::Stamp;
use crate::db::{ClockId, GroupId};
use crate::encoding;
use crate::model::{Declaration, ModelId, Operation};

/// Names an entry, or a payload, by its content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct Hash([u8; 32]);

/// One entry in one author's chain: where it is, when it was, and which
/// one it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Link {
    pub(crate) seq: u64,
    pub(crate) lamport: u64,
    pub(crate) hash: Hash,
}

/// How far along each author's chain something reaches.
pub(crate) type Frontier = BTreeMap<Peer, Link>;

/// How far along `author`'s chain `frontier` reaches. Zero if nowhere.
pub(crate) fn reach(frontier: &Frontier, author: Peer) -> u64 {
    frontier.get(&author).map_or(0, |link| link.seq)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Header {
    /// So a signed entry can never be replayed into another group.
    pub(crate) group: GroupId,
    /// The clock it's part of. Its author has one chain in each.
    pub(crate) clock: ClockId,
    /// Its place in its author's chain, counting from one.
    pub(crate) seq: u64,
    /// The entry before it in its author's chain.
    pub(crate) previous: Option<Hash>,
    /// The heads of the other chains its author had taken in.
    pub(crate) after: Frontier,
    /// Greater than that of everything it was written on top of.
    pub(crate) lamport: u64,
    /// The latest checkpoint its author held.
    pub(crate) under: Under,
    pub(crate) payload: Hash,
}

/// The checkpoint an entry was written under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Under {
    pub(crate) height: u64,
    pub(crate) checkpoint: Digest,
}

impl Signable for Header {
    const CONTEXT: &'static [u8] = b"woodland/entry";
}

/// An entry as it travels and as it's stored. Not yet checked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub(crate) header: Signed<Header>,
    pub(crate) payload: Vec<u8>,
}

/// An entry whose signature and payload have been checked.
#[derive(Clone, Debug)]
pub(crate) struct Checked {
    pub(crate) entry: Entry,
    pub(crate) operations: Vec<Operation>,
    hash: Hash,
}

impl Entry {
    /// Signs a new entry.
    pub(crate) fn author(
        identity: &Identity,
        mut header: Header,
        operations: Vec<Operation>,
    ) -> Checked {
        let payload = encoding::encode(&operations);
        header.payload = Hash::of_payload(&payload);
        let hash = Hash::of_header(identity.peer(), &header);
        let header = Signed::sign(identity, header).signed().clone();

        Checked {
            entry: Self { header, payload },
            operations,
            hash,
        }
    }

    /// Checks everything that can be checked without knowing anything
    /// else: the signature, the group, that the payload is the one the
    /// header was signed over, and that it stays within its clock.
    pub(crate) fn check(self, group: GroupId) -> Option<Checked> {
        let header = verify_all(vec![self.header.clone()]).pop()?;
        let said = header.payload();
        let intact = said.group == group && said.payload == Hash::of_payload(&self.payload);
        let operations: Vec<Operation> = encoding::decode(&self.payload)?;
        let within = |operation: &Operation| operation.model.clock() == said.clock;
        let intact = intact && operations.iter().all(within);
        let hash = Hash::of_header(header.author(), said);

        intact.then_some(Checked {
            entry: self,
            operations,
            hash,
        })
    }
}

impl Entry {
    /// Where the entry says it sits. Nothing about it has been checked.
    pub(crate) fn link(&self) -> Link {
        let header = self.header.payload();

        Link {
            seq: header.seq,
            lamport: header.lamport,
            hash: Hash::of_header(self.header.author(), header),
        }
    }

    /// Reads back an entry that was checked before it was stored.
    pub(crate) fn trusted(self) -> Option<Checked> {
        let operations = encoding::decode(&self.payload)?;
        let hash = Hash::of_header(self.header.author(), self.header.payload());

        Some(Checked {
            entry: self,
            operations,
            hash,
        })
    }
}

impl Hash {
    /// For a header that hasn't been filled in yet.
    pub(crate) const NONE: Self = Self([0; 32]);

    fn of_payload(payload: &[u8]) -> Self {
        Self(encoding::hash("woodland/entry/payload", &payload))
    }

    /// The signature is left out: it's the author and what they said
    /// that name an entry, not how it was signed.
    fn of_header(author: Peer, header: &Header) -> Self {
        Self(encoding::hash("woodland/entry/header", &(author, header)))
    }
}

impl Checked {
    pub(crate) fn author(&self) -> Peer {
        self.entry.header.author()
    }

    pub(crate) fn header(&self) -> &Header {
        self.entry.header.payload()
    }

    pub(crate) fn link(&self) -> Link {
        let header = self.header();

        Link {
            seq: header.seq,
            lamport: header.lamport,
            hash: self.hash,
        }
    }

    pub(crate) fn outline(&self) -> Outline {
        let operations = self.operations.iter();
        let waits = operations
            .clone()
            .filter(|it| it.waits().is_some() || it.drops());

        Outline {
            author: self.author(),
            link: self.link(),
            after: self.header().after.clone(),
            under: self.header().under.height,
            declared: operations
                .flat_map(|it| it.declares(self.author()))
                .collect(),
            waits: waits.map(|it| it.model).collect(),
        }
    }
}

/// What's remembered of an entry that hasn't been committed: where it
/// sits, and what its operations declare. No values. The entry itself is
/// in the store.
#[derive(Clone, Debug)]
pub(crate) struct Outline {
    pub(crate) author: Peer,
    pub(crate) link: Link,
    pub(crate) after: Frontier,
    /// The height of the checkpoint it was written under.
    pub(crate) under: u64,
    pub(crate) declared: Vec<Declaration>,
    /// The models it has changes for that wait to be committed. Those
    /// declare nothing.
    pub(crate) waits: BTreeSet<ModelId>,
}

impl Outline {
    pub(crate) fn stamp(&self) -> Stamp {
        Stamp {
            lamport: self.link.lamport,
            author: self.author,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::{Checkpoint, Electorate};
    use crate::model::ModelId;

    fn entry(clock: ClockId, model: ModelId) -> (GroupId, Entry) {
        let identity = Identity::from_secret_key([1; 32]);
        let founding = Checkpoint::founding([2; 32], Electorate::new([(identity.peer(), 1)]));
        let group = GroupId::from_bytes(*founding.digest().as_bytes());
        let header = Header {
            group,
            clock,
            seq: 1,
            previous: None,
            after: Frontier::new(),
            lamport: 1,
            under: Under {
                height: 0,
                checkpoint: founding.digest(),
            },
            payload: Hash::NONE,
        };
        let operations = vec![Operation::set(model, b"key".to_vec(), b"value".to_vec())];

        (group, Entry::author(&identity, header, operations).entry)
    }

    #[test]
    fn an_entry_writes_only_to_its_own_clock() {
        let clock = ClockId::from_bytes([3; 32]);
        let other = ClockId::from_bytes([4; 32]);

        let (group, within) = entry(clock, ModelId::kv(clock, [5; 32]));
        assert!(within.check(group).is_some());

        let (group, outside) = entry(clock, ModelId::kv(other, [5; 32]));
        assert!(outside.check(group).is_none());
    }

    #[test]
    fn an_entry_is_no_good_in_another_group() {
        let clock = ClockId::from_bytes([3; 32]);
        let (_, entry) = entry(clock, ModelId::kv(clock, [5; 32]));

        assert!(entry.check(GroupId::from_bytes([9; 32])).is_none());
    }
}
