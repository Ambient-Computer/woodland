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

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::actor::{Identity, Peer, Signable, Signed, verify_all};
use crate::agreement::AgreementId;
use crate::chain::Digest;
use crate::clock::Stamp;
use crate::encoding;
use crate::model::{Declaration, Operation};

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
    pub(crate) group: AgreementId,
    /// Its place in its author's chain, counting from one.
    pub(crate) seq: u64,
    /// The entry before it in its author's chain.
    pub(crate) previous: Option<Hash>,
    /// The heads of the other chains its author had taken in.
    pub(crate) after: Frontier,
    /// Greater than that of everything it was written on top of.
    pub(crate) lamport: u64,
    /// The latest checkpoint its author held.
    pub(crate) checkpoint: Digest,
    pub(crate) payload: Hash,
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
    /// else: the signature, the group, and that the payload is the one
    /// the header was signed over.
    pub(crate) fn check(self, group: AgreementId) -> Option<Checked> {
        let header = verify_all(vec![self.header.clone()]).pop()?;
        let said = header.payload();
        let intact = said.group == group && said.payload == Hash::of_payload(&self.payload);
        let operations = encoding::decode(&self.payload)?;
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

        Outline {
            author: self.author(),
            link: self.link(),
            after: self.header().after.clone(),
            declared: operations.flat_map(Operation::declares).collect(),
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
    pub(crate) declared: Vec<Declaration>,
}

impl Outline {
    pub(crate) fn stamp(&self) -> Stamp {
        Stamp {
            lamport: self.link.lamport,
            author: self.author,
        }
    }
}
