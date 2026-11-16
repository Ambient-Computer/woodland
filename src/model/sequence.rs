//! Sequences: ordered lists that merge.
//!
//! Every other kind of model replaces: a key holds what the last
//! transaction to write it left there. A sequence combines. A key
//! holds a list, a transaction edits it, and what's read is every
//! edit applied in timeline order. Two devices that each add an item
//! while apart find both there when they meet.
//!
//! # Edits
//!
//! - **Insert** puts a new item between two others its author read.
//! - **Remove** takes an item out.
//! - **Move** puts an item somewhere else. Of two concurrent moves of
//!   one item, the later in the timeline decides where it is, and it's
//!   never in two places.
//!
//! An edit says where by naming the items on either side, not by
//! counting. If the one before is gone by the time the edit applies,
//! it goes ahead of the one after. If both are gone, it goes at the
//! end. So no edit is ever pruned for what another did, and none needs
//! anything kept of an item once it's removed: a committed list is
//! its items and nothing else.
//!
//! Items put in right after the same item read newest first, so a run
//! typed on one device stays together whatever was typed on another.
//!
//! # Size
//!
//! A list is one value, read and written whole. It suits a list of
//! tasks or the paragraphs of a note. A long text is better kept as a
//! sequence of pieces, each its own key or row.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::ModelId;
use crate::actor::Peer;
use crate::clock::Stamp;
use crate::db::Transaction;
use crate::encoding;

/// Names an item for as long as it's in its list.
pub(crate) type ItemId = [u8; 16];

/// A list as it's kept: its items, in order.
type Items = Vec<(ItemId, Vec<u8>)>;

/// An item an edit is placed next to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Anchor {
    Item(ItemId),
    /// The item put in by an earlier operation of the same
    /// transaction: this one, counting from the first. It's replaced
    /// by that item's ID when the transaction is written.
    Own(u32),
}

/// Where an edit puts an item: after one, or failing that before
/// another, or failing that at the end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Between {
    after: Option<Anchor>,
    before: Option<Anchor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Edit {
    /// The item's ID is filled in when the transaction is written.
    Insert {
        id: ItemId,
        at: Between,
        value: Vec<u8>,
    },
    Remove {
        id: ItemId,
    },
    Move {
        id: ItemId,
        at: Between,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Operation {
    key: Vec<u8>,
    edit: Edit,
}

fn id_of(author: Peer, seq: u64, index: usize) -> ItemId {
    let hash = encoding::hash("woodland/item", &(author, seq, index));
    let mut id = ItemId::default();
    id.copy_from_slice(&hash[..16]);

    id
}

impl Operation {
    pub(crate) fn new(key: Vec<u8>, edit: Edit) -> Self {
        Self { key, edit }
    }

    /// Gives an insert the ID of its item: one no other item has,
    /// because no other operation is this one. And points what was
    /// placed next to an item of the same transaction at that item.
    pub(crate) fn seal(&mut self, author: Peer, seq: u64, index: usize) {
        let own = |anchor: &mut Option<Anchor>| {
            if let Some(Anchor::Own(index)) = *anchor {
                let index = usize::try_from(index).unwrap_or(usize::MAX);
                *anchor = Some(Anchor::Item(id_of(author, seq, index)));
            }
        };

        match &mut self.edit {
            Edit::Insert { id, at, .. } => {
                *id = id_of(author, seq, index);
                own(&mut at.after);
                own(&mut at.before);
            }

            Edit::Move { at, .. } => {
                own(&mut at.after);
                own(&mut at.before);
            }

            Edit::Remove { .. } => {}
        }
    }

    /// The list's key, and the edit to combine with what it holds.
    pub(crate) fn write(&self) -> (Vec<u8>, Option<Vec<u8>>) {
        (self.key.clone(), Some(encoding::encode(&self.edit)))
    }
}

fn place(items: &Items, at: Between) -> usize {
    let find = |anchor: Option<Anchor>| {
        let Some(Anchor::Item(wanted)) = anchor else {
            return None;
        };

        items.iter().position(|(id, _)| *id == wanted)
    };
    let after = find(at.after).map(|index| index + 1);

    after.or_else(|| find(at.before)).unwrap_or(items.len())
}

fn apply(items: &mut Items, edit: Edit) {
    let held = |items: &Items, wanted: ItemId| items.iter().position(|(id, _)| *id == wanted);

    match edit {
        Edit::Insert { id, at, value } => {
            if held(items, id).is_none() {
                items.insert(place(items, at), (id, value));
            }
        }

        Edit::Remove { id } => items.retain(|(held, _)| *held != id),

        Edit::Move { id, at } => {
            if let Some(index) = held(items, id) {
                let item = items.remove(index);
                items.insert(place(items, at), item);
            }
        }
    }
}

/// What a list holds once `edit` is applied to it. An edit that isn't
/// one changes nothing.
pub(crate) fn combine(list: Option<&[u8]>, edit: &[u8]) -> Vec<u8> {
    let mut items: Items = list.and_then(encoding::decode).unwrap_or_default();
    if let Some(edit) = encoding::decode(edit) {
        apply(&mut items, edit);
    }

    encoding::encode(&items)
}

/// Every key of a merging model as it reads: what's committed, with
/// each write that isn't yet combined with it in timeline order.
pub(crate) fn current(
    committed: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
    versions: impl IntoIterator<Item = (Vec<u8>, Stamp, Option<Vec<u8>>)>,
    combine: impl Fn(Option<&[u8]>, &[u8]) -> Vec<u8>,
) -> BTreeMap<Vec<u8>, Vec<u8>> {
    let mut lists: BTreeMap<Vec<u8>, Vec<u8>> = committed.into_iter().collect();
    let mut edits: Vec<_> = versions.into_iter().collect();
    edits.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    for (key, _, edits) in edits {
        // Everything one transaction did to the list, in order.
        let edits = edits.and_then(|edits| encoding::decode::<Vec<Vec<u8>>>(&edits));
        for edit in edits.unwrap_or_default() {
            let list = combine(lists.get(&key).map(Vec::as_slice), &edit);
            lists.insert(key.clone(), list);
        }
    }

    lists
}

/// A list read from a [sequence] model.
///
/// [sequence]: ModelId::sequence
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sequence {
    items: Items,
}

impl Sequence {
    /// The list under `key`, from the results of a query that covers
    /// it. Empty if there isn't one.
    #[must_use]
    pub fn read(results: &[(Vec<u8>, Vec<u8>)], key: impl AsRef<[u8]>) -> Self {
        let found = results.iter().find(|(held, _)| held == key.as_ref());
        let items = found.and_then(|(_, list)| encoding::decode(list));

        Self {
            items: items.unwrap_or_default(),
        }
    }

    /// Its items, in order.
    pub fn values(&self) -> impl Iterator<Item = &[u8]> {
        self.items.iter().map(|(_, value)| value.as_slice())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn anchor(&self, index: Option<usize>) -> Option<Anchor> {
        let (id, _) = self.items.get(index?)?;

        Some(Anchor::Item(*id))
    }

    /// Between the items either side of `index`, leaving out the item
    /// at `without` as if it weren't there.
    fn between(&self, index: usize, without: Option<usize>) -> Between {
        let mut kept = (0..self.items.len()).filter(|it| Some(*it) != without);
        let index = index.min(self.items.len());
        let before = kept.clone().nth(index);
        let after = index.checked_sub(1).and_then(|it| kept.nth(it));

        Between {
            after: self.anchor(after),
            before: self.anchor(before),
        }
    }
}

impl Transaction {
    /// Puts `values` into the list under `key` of a [sequence] model,
    /// one after another, so that the first is at `index` of the list
    /// as `read`. Past the end means at the end. Naming a new model
    /// or a new key creates it.
    ///
    /// What others put into the list meanwhile stays, and so does
    /// this: see [`ModelId::sequence`].
    ///
    /// [sequence]: ModelId::sequence
    pub fn insert_at<T: Into<Vec<u8>>>(
        mut self,
        model: ModelId,
        key: impl Into<Vec<u8>>,
        read: &Sequence,
        index: usize,
        values: impl IntoIterator<Item = T>,
    ) -> Self {
        let key = key.into();
        let mut at = read.between(index, None);
        for value in values {
            let own = u32::try_from(self.len()).unwrap_or(u32::MAX);
            let edit = Edit::Insert {
                id: ItemId::default(),
                at,
                value: value.into(),
            };
            self = self.with(super::Operation::sequence(model, key.clone(), edit));
            at.after = Some(Anchor::Own(own));
        }

        self
    }

    /// Takes the item at `index` of the list as `read` out of it,
    /// wherever it is by then. Does nothing if there's no such item.
    pub fn remove_at(
        self,
        model: ModelId,
        key: impl Into<Vec<u8>>,
        read: &Sequence,
        index: usize,
    ) -> Self {
        let Some((id, _)) = read.items.get(index) else {
            return self;
        };
        let edit = Edit::Remove { id: *id };

        self.with(super::Operation::sequence(model, key.into(), edit))
    }

    /// Moves the item at `from` of the list as `read`, so that it's at
    /// `to` once it has. If someone else moves it too, it ends up
    /// where one of them put it.
    pub fn move_to(
        self,
        model: ModelId,
        key: impl Into<Vec<u8>>,
        read: &Sequence,
        from: usize,
        to: usize,
    ) -> Self {
        let Some((id, _)) = read.items.get(from) else {
            return self;
        };
        let edit = Edit::Move {
            id: *id,
            at: read.between(to, Some(from)),
        };

        self.with(super::Operation::sequence(model, key.into(), edit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::peers::{laptop, phone};

    fn insert(author: Peer, seq: u64, at: Between, value: &str) -> Edit {
        Edit::Insert {
            id: id_of(author, seq, 0),
            at,
            value: value.into(),
        }
    }

    fn after(author: Peer, seq: u64) -> Between {
        Between {
            after: Some(Anchor::Item(id_of(author, seq, 0))),
            before: None,
        }
    }

    fn read(items: &Items) -> Vec<String> {
        let text = |(_, value): &(ItemId, Vec<u8>)| String::from_utf8_lossy(value).into_owned();

        items.iter().map(text).collect()
    }

    fn all(edits: impl IntoIterator<Item = Edit>) -> Items {
        let mut items = Items::new();
        for edit in edits {
            apply(&mut items, edit);
        }

        items
    }

    #[test]
    fn runs_typed_apart_dont_interleave() {
        let start = Between::default();
        let items = all([
            insert(phone(), 1, start, "a"),
            insert(laptop(), 1, start, "x"),
            insert(phone(), 2, after(phone(), 1), "b"),
            insert(laptop(), 2, after(laptop(), 1), "y"),
        ]);

        assert_eq!(read(&items), ["a", "b", "x", "y"]);
    }

    #[test]
    fn an_edit_next_to_something_removed_stays_close() {
        let middle = Between {
            after: Some(Anchor::Item(id_of(phone(), 2, 0))),
            before: Some(Anchor::Item(id_of(phone(), 3, 0))),
        };
        let items = all([
            insert(phone(), 1, Between::default(), "a"),
            insert(phone(), 2, after(phone(), 1), "b"),
            insert(phone(), 3, after(phone(), 2), "c"),
            Edit::Remove {
                id: id_of(phone(), 2, 0),
            },
            insert(laptop(), 1, middle, "x"),
        ]);
        assert_eq!(read(&items), ["a", "x", "c"]);

        let nowhere = after(laptop(), 9);
        let items = all([
            insert(phone(), 1, Between::default(), "a"),
            insert(laptop(), 1, nowhere, "x"),
        ]);
        assert_eq!(read(&items), ["a", "x"]);
    }

    #[test]
    fn an_item_moved_twice_is_in_one_place() {
        let first = Edit::Move {
            id: id_of(phone(), 1, 0),
            at: after(phone(), 3),
        };
        let second = Edit::Move {
            id: id_of(phone(), 1, 0),
            at: after(phone(), 2),
        };
        let gone = Edit::Move {
            id: id_of(laptop(), 9, 0),
            at: Between::default(),
        };
        let items = all([
            insert(phone(), 1, Between::default(), "a"),
            insert(phone(), 2, after(phone(), 1), "b"),
            insert(phone(), 3, after(phone(), 2), "c"),
            first,
            second,
            gone,
        ]);

        assert_eq!(read(&items), ["b", "a", "c"]);
    }

    #[test]
    fn edits_apply_in_timeline_order_however_theyre_handed_over() {
        let version = |lamport: u64, edit: Edit| {
            let stamp = Stamp {
                lamport,
                author: phone(),
            };
            let edits = vec![encoding::encode(&edit)];

            (b"list".to_vec(), stamp, Some(encoding::encode(&edits)))
        };
        let first = version(1, insert(phone(), 1, Between::default(), "a"));
        let second = version(2, insert(phone(), 2, after(phone(), 1), "b"));

        let lists = current([], [second, first], combine);
        let items: Items = encoding::decode(&lists[&b"list".to_vec()]).unwrap_or_default();

        assert_eq!(read(&items), ["a", "b"]);
    }

    #[test]
    fn an_item_is_put_in_once() {
        let twice = insert(phone(), 1, Between::default(), "a");

        assert_eq!(read(&all([twice.clone(), twice])), ["a"]);
    }

    #[test]
    fn nonsense_changes_nothing() {
        let list = encoding::encode(&all([insert(phone(), 1, Between::default(), "a")]));

        assert_eq!(combine(Some(&list), b"\xFF\xFF"), list);
    }
}
