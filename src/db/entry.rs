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
use crate::seal::{Keyring, Opened};

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
    /// What it does. Nothing, if this device can't tell.
    pub(crate) operations: Vec<Operation>,
    /// Whether this device can't tell what it does: it's sealed under
    /// a secret that isn't held here.
    pub(crate) unread: bool,
    hash: Hash,
}

/// What an entry's payload is sealed about: where the entry sits. A
/// payload moved to another entry doesn't open.
fn about(author: Peer, header: &Header) -> Vec<u8> {
    encoding::encode(&(header.group, header.clock, author, header.seq))
}

impl Entry {
    /// Signs a new entry. `None` if the group is sealed and this
    /// device lacks the secret to write under.
    pub(crate) fn author(
        identity: &Identity,
        mut header: Header,
        operations: Vec<Operation>,
        keys: &Keyring,
    ) -> Option<Checked> {
        let about = about(identity.peer(), &header);
        let plain = encoding::encode(&carried(&operations));
        let payload = keys.seal(&about, header.under.height, plain)?;
        header.payload = Hash::of_payload(&payload);
        let hash = Hash::of_header(identity.peer(), &header);
        let header = Signed::sign(identity, header).signed().clone();

        Some(Checked {
            entry: Self { header, payload },
            operations,
            unread: false,
            hash,
        })
    }

    /// Checks everything that can be checked without knowing anything
    /// else: the signature, the group, that the payload is the one the
    /// header was signed over, and, if this device can read it, that
    /// it says something and stays within its clock.
    ///
    /// An entry this device lacks the secret for is still an entry.
    /// It's kept and passed on as it is. One that everyone with the
    /// secret can see is nonsense isn't.
    pub(crate) fn check(self, group: GroupId, keys: &Keyring) -> Option<Checked> {
        let header = verify_all(vec![self.header.clone()]).pop()?;
        let said = header.payload();
        let intact = said.group == group && said.payload == Hash::of_payload(&self.payload);
        let (operations, unread) = match self.read(keys) {
            Read::Does(operations) => (operations, false),
            Read::Sealed | Read::Foreign => (Vec::new(), true),
            Read::Nonsense => return None,
        };
        let within = |operation: &Operation| operation.model.clock() == said.clock;
        let intact = intact && operations.iter().all(within);
        let hash = Hash::of_header(header.author(), said);

        intact.then_some(Checked {
            entry: self,
            operations,
            unread,
            hash,
        })
    }

    fn read(&self, keys: &Keyring) -> Read {
        let header = self.header.payload();
        let about = about(self.header.author(), header);

        match keys.open(&about, header.under.height, &self.payload) {
            Opened::Plain(plain) => understood(&plain, header.clock),
            Opened::Unreadable => Read::Sealed,
            Opened::Garbled => Read::Nonsense,
        }
    }
}

/// An operation as it's written down. What it changes is in the
/// vocabulary of its model's kind, and a device that lacks the kind
/// can still tell where one operation ends and the next begins.
#[derive(Serialize, Deserialize)]
struct Carried {
    model: ModelId,
    /// The version of the library that first had the operation, if
    /// that's later than the one that first had its kind.
    since: u8,
    change: Vec<u8>,
}

fn carried(operations: &[Operation]) -> Vec<Carried> {
    let carried = |operation: &Operation| Carried {
        model: operation.model,
        since: operation.since(),
        change: encoding::encode(&operation.change),
    };

    operations.iter().map(carried).collect()
}

/// What `plain` says, to a device that has the kinds it's for.
fn understood(plain: &[u8], clock: ClockId) -> Read {
    let Some(carried) = encoding::decode::<Vec<Carried>>(plain) else {
        return Read::Nonsense;
    };
    if carried.iter().any(|it| it.model.clock() != clock) {
        return Read::Nonsense;
    }
    let latest = crate::model::vocabulary();
    if carried
        .iter()
        .any(|it| it.model.kind().is_none() || it.since > latest)
    {
        return Read::Foreign;
    }

    let operation = |carried: Carried| {
        let change = encoding::decode(&carried.change)?;
        let operation = Operation {
            model: carried.model,
            change,
        };

        // A version that can read it has to agree on when it was
        // added, or an earlier one couldn't tell it from nonsense.
        (operation.since() == carried.since).then_some(operation)
    };
    let operations: Option<Vec<Operation>> = carried.into_iter().map(operation).collect();

    operations.map_or(Read::Nonsense, Read::Does)
}

/// What a device makes of an entry's payload.
enum Read {
    Does(Vec<Operation>),
    /// This device lacks the secret to tell.
    Sealed,
    /// It's for a kind of model this device doesn't have, or says
    /// something a later version of the library added to one it has.
    Foreign,
    /// It isn't a list of operations, whoever reads it.
    Nonsense,
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
    pub(crate) fn trusted(self, keys: &Keyring) -> Checked {
        let hash = Hash::of_header(self.header.author(), self.header.payload());
        let (operations, unread) = match self.read(keys) {
            Read::Does(operations) => (operations, false),
            Read::Sealed | Read::Foreign => (Vec::new(), true),
            Read::Nonsense => (Vec::new(), false),
        };

        Checked {
            entry: self,
            operations,
            unread,
            hash,
        }
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
            unread: self.unread,
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
    /// Whether this device can't tell what it does.
    pub(crate) unread: bool,
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
    use crate::seal::Secret;

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

        let entry = Entry::author(&identity, header, operations, &Keyring::Open);

        (group, entry.expect("an open group").entry)
    }

    /// An entry like [`entry`]'s, saying `plain`.
    fn saying(clock: ClockId, plain: &[u8]) -> (GroupId, Entry) {
        let identity = Identity::from_secret_key([1; 32]);
        let (group, entry) = entry(clock, ModelId::kv(clock, [5; 32]));
        let mut header = entry.header.payload().clone();
        header.payload = Hash::of_payload(plain);
        let header = Signed::sign(&identity, header).signed().clone();
        let payload = plain.to_vec();

        (group, Entry { header, payload })
    }

    #[test]
    fn an_entry_for_a_kind_this_device_lacks_is_kept_unread() {
        let clock = ClockId::from_bytes([3; 32]);
        let other = ClockId::from_bytes([4; 32]);
        let (group, newer) = entry(clock, ModelId::allowances(clock, [5; 32]));
        let (_, outside) = entry(clock, ModelId::allowances(other, [5; 32]));

        crate::model::predate(true);
        let kept = newer.clone().check(group, &Keyring::Open);
        let refused = outside.check(group, &Keyring::Open);
        crate::model::predate(false);

        assert!(kept.expect("an entry all the same").unread);
        assert!(refused.is_none(), "it says which clock it's for");
        assert!(!newer.trusted(&Keyring::Open).unread);
    }

    #[test]
    fn an_entry_that_says_nothing_its_kind_could_mean_is_refused() {
        let clock = ClockId::from_bytes([3; 32]);
        let model = ModelId::kv(clock, [5; 32]);
        let sound = carried(&[Operation::set(model, b"key".to_vec(), b"value".to_vec())]);
        let garbled = vec![Carried {
            model,
            since: 0,
            change: vec![0xFF; 9],
        }];

        let (group, entry) = saying(clock, &encoding::encode(&sound));
        assert!(entry.check(group, &Keyring::Open).is_some());
        let (group, entry) = saying(clock, &encoding::encode(&garbled));
        assert!(entry.check(group, &Keyring::Open).is_none());
        let (group, entry) = saying(clock, b"\xFF\xFF\xFF");
        assert!(entry.check(group, &Keyring::Open).is_none());
    }

    /// A later version can add an operation to a kind an earlier one
    /// has. The earlier one can tell that's what it's looking at.
    #[test]
    fn an_operation_added_to_a_kind_since_is_kept_unread() {
        let clock = ClockId::from_bytes([3; 32]);
        let table = ModelId::table(clock, [5; 32]);
        let name = crate::model::table::Operation::Name {
            name: "people".to_owned(),
        };
        let mut said = carried(&[Operation::table(table, name)]);
        let (group, newer) = saying(clock, &encoding::encode(&said));

        crate::model::predate(true);
        let kept = newer.clone().check(group, &Keyring::Open);
        crate::model::predate(false);

        assert!(kept.expect("an entry all the same").unread);
        assert!(!newer.check(group, &Keyring::Open).expect("read").unread);

        // One that claims to be older than it is would be nonsense to
        // a device that old, so it's nonsense to every device.
        said[0].since = 0;
        let (group, lying) = saying(clock, &encoding::encode(&said));
        assert!(lying.check(group, &Keyring::Open).is_none());
    }

    #[test]
    fn an_entry_writes_only_to_its_own_clock() {
        let clock = ClockId::from_bytes([3; 32]);
        let other = ClockId::from_bytes([4; 32]);

        let (group, within) = entry(clock, ModelId::kv(clock, [5; 32]));
        assert!(within.check(group, &Keyring::Open).is_some());

        let (group, outside) = entry(clock, ModelId::kv(other, [5; 32]));
        assert!(outside.check(group, &Keyring::Open).is_none());
    }

    fn sealed_with(secret: Secret) -> Keyring {
        let mut keys = Keyring::sealed();
        keys.hold(0, secret);

        keys
    }

    fn sealed_under(purpose: &[u8]) -> Keyring {
        let secret = Secret::make(&Identity::from_secret_key([1; 32]), purpose);

        sealed_with(secret)
    }

    fn sealed_entry(keys: &Keyring) -> (GroupId, Entry) {
        let clock = ClockId::from_bytes([3; 32]);
        let (group, entry) = entry(clock, ModelId::kv(clock, [5; 32]));
        let header = entry.header.payload().clone();
        let identity = Identity::from_secret_key([1; 32]);
        let operations = vec![Operation::set(
            ModelId::kv(clock, [5; 32]),
            b"key".to_vec(),
            b"value".to_vec(),
        )];
        let entry = Entry::author(&identity, header, operations, keys);

        (group, entry.expect("the secret is held").entry)
    }

    #[test]
    fn a_sealed_entry_is_read_with_its_secret_and_kept_without() {
        let keys = sealed_under(b"ours");
        let (group, entry) = sealed_entry(&keys);
        let without = Keyring::sealed();

        let read = entry.clone().check(group, &keys).expect("an entry");
        assert!(!read.unread);
        assert_eq!(read.operations.len(), 1);

        let carried = entry.clone().check(group, &without).expect("an entry");
        assert!(carried.unread);
        assert_eq!(carried.operations.len(), 0);
        assert!(entry.trusted(&without).unread);
    }

    /// Whoever holds the secret can tell, and they all tell the same.
    #[test]
    fn an_entry_that_isnt_sealed_under_the_groups_secret_is_no_entry() {
        let (group, entry) = sealed_entry(&sealed_under(b"theirs"));

        assert!(entry.check(group, &sealed_under(b"ours")).is_none());
    }

    #[test]
    fn a_sealed_entry_doesnt_open_anywhere_else() {
        let keys = sealed_under(b"ours");
        let (group, entry) = sealed_entry(&keys);
        let header = entry.header.payload().clone();
        let identity = Identity::from_secret_key([1; 32]);
        let moved = Header {
            seq: 2,
            payload: Hash::of_payload(&entry.payload),
            ..header
        };
        let moved = Entry {
            header: Signed::sign(&identity, moved).signed().clone(),
            payload: entry.payload,
        };

        assert!(moved.check(group, &keys).is_none());
    }

    #[test]
    fn nothing_is_written_without_the_secret_in_force() {
        let clock = ClockId::from_bytes([3; 32]);
        let (_, entry) = entry(clock, ModelId::kv(clock, [5; 32]));
        let header = entry.header.payload().clone();
        let identity = Identity::from_secret_key([1; 32]);
        let secret = Secret::make(&identity, b"old");
        let keys = sealed_with(secret);
        let later = Keyring::sealed();

        assert!(Entry::author(&identity, header.clone(), Vec::new(), &keys).is_some());
        assert!(Entry::author(&identity, header, Vec::new(), &later).is_none());
    }

    #[test]
    fn an_entry_is_no_good_in_another_group() {
        let clock = ClockId::from_bytes([3; 32]);
        let (_, entry) = entry(clock, ModelId::kv(clock, [5; 32]));

        assert!(
            entry
                .check(GroupId::from_bytes([9; 32]), &Keyring::Open)
                .is_none()
        );
    }
}
