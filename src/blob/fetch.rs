//! A blob in parts: what one device asks another for, and how what
//! comes back is checked.
//!
//! A blob is fetched top down. The hashes of its pages are checked
//! against its name, a page of its groups' hashes against one of
//! those, and a piece of its bytes against the hashes of the groups
//! in it. Each part is checked before it's stored, so nothing here
//! is ever held that isn't part of the blob it's filed under.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use serde::{Deserialize, Serialize};

use super::hash::{self, GROUP, PAGE};
use super::kept::{self, GROUPS, Kept, PIECE, size};
use super::{Listed, keys, tally};
use crate::db::GroupId;
use crate::db::step::Step;
use crate::store::{StoreError, View};

/// One part of a blob, small enough to send at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Part {
    /// The hash of each page of its groups' hashes. Only a blob of
    /// more than one page has them.
    Top,
    /// One page of its groups' hashes. Only a blob of more than one
    /// group has them.
    Leaves(u64),
    /// One piece of its bytes, as they're stored.
    Piece(u64),
}

/// What's said about blobs, apart from everything else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Message {
    /// Asks for a part of a blob that `group` refers to.
    Ask {
        group: GroupId,
        blob: Listed,
        part: Part,
    },
    Have {
        blob: Listed,
        part: Part,
        bytes: Vec<u8>,
    },
    /// The part isn't to be had from here.
    Lacks { blob: Listed, part: Part },
}

/// How many pages of hashes a blob of `groups` groups has.
fn pages(groups: u64) -> u64 {
    groups.div_ceil(PAGE)
}

/// How many bytes `part` of `blob` is, if it has such a part.
fn length(blob: Listed, part: Part) -> Option<u64> {
    let groups = hash::groups(blob.length);
    match part {
        Part::Top => (groups > PAGE).then(|| pages(groups) * 32),
        Part::Leaves(page) => {
            let within = groups > 1 && page < pages(groups);

            within.then(|| (groups - page * PAGE).min(PAGE) * 32)
        }
        Part::Piece(index) => {
            let start = index.checked_mul(PIECE)?;

            (start < blob.length).then(|| (blob.length - start).min(PIECE))
        }
    }
}

/// The groups a piece is made of.
fn groups_of(blob: Listed, index: u64) -> (u64, u64) {
    let first = index * GROUPS;

    (first, (first + GROUPS).min(hash::groups(blob.length)))
}

/// A part of `blob` to send, if all of it is held.
pub(crate) async fn serve<V: View>(
    step: &Step<V>,
    blob: Listed,
    part: Part,
) -> Result<Option<Vec<u8>>, StoreError> {
    let (Some((number, kept)), Some(length)) = (kept::find(step, blob).await?, length(blob, part))
    else {
        return Ok(None);
    };

    let key = match part {
        Part::Top => keys::top(number),
        Part::Leaves(page) => keys::leaf_page(number, page),
        Part::Piece(index) => {
            let (first, last) = groups_of(blob, index);
            if !kept.covers(first * GROUP, (last * GROUP).min(blob.length)) {
                return Ok(None);
            }

            keys::piece(number, index)
        }
    };
    let stored = step.get(&key).await?;

    Ok(stored.filter(|bytes| bytes.len() as u64 == length))
}

/// What became of a part that arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Accepted {
    /// It wasn't part of the blob, or wasn't wanted.
    Dropped,
    Stored,
    /// It was the last of the blob.
    Whole,
}

/// The hash each of a piece's groups should have, if the pages that
/// say so are here.
async fn expected<V: View>(
    step: &Step<V>,
    number: u64,
    (first, last): (u64, u64),
) -> Result<Option<Vec<[u8; 32]>>, StoreError> {
    let mut found = Vec::new();
    let mut pages = BTreeMap::new();
    for group in first..last {
        let page = group / PAGE;
        if let Entry::Vacant(vacant) = pages.entry(page) {
            vacant.insert(kept::hashes(step, &keys::leaf_page(number, page)).await?);
        }
        let Some(hash) = pages[&page].get(size(group % PAGE)) else {
            return Ok(None);
        };

        found.push(*hash);
    }

    Ok(Some(found))
}

/// Whether `bytes` is `part` of `blob`, going by what's already been
/// checked.
async fn checks<V: View>(
    step: &Step<V>,
    number: u64,
    blob: Listed,
    part: Part,
    bytes: &[u8],
) -> Result<bool, StoreError> {
    let groups = hash::groups(blob.length);
    let hashes = bytes.as_chunks::<32>().0;
    match part {
        Part::Top => Ok(hash::root(hashes) == blob.name),
        Part::Leaves(_) if groups <= PAGE => Ok(hash::root(hashes) == blob.name),
        Part::Leaves(page) => {
            let top = kept::hashes(step, &keys::top(number)).await?;

            Ok(top.get(size(page)) == Some(&hash::subtree(hashes)))
        }
        Part::Piece(_) if groups == 1 => Ok(hash::small(bytes) == blob.name),
        Part::Piece(index) => {
            let (first, last) = groups_of(blob, index);
            let Some(expected) = expected(step, number, (first, last)).await? else {
                return Ok(false);
            };
            let pieces = bytes.chunks(size(GROUP));
            let offsets = (first..last).map(|group| group * GROUP);
            let found = offsets
                .zip(pieces)
                .map(|(offset, piece)| hash::group(offset, piece));

            Ok(found.eq(expected))
        }
    }
}

/// Adds the groups `first..last` to what's held.
fn hold(kept: &mut Kept, (first, last): (u64, u64)) {
    kept.held.push((first, last));
    kept.held.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for &(start, end) in &kept.held {
        match merged.last_mut() {
            Some(before) if start <= before.1 => before.1 = before.1.max(end),
            _ => merged.push((start, end)),
        }
    }

    kept.held = merged;
}

/// Takes in a part of a blob that arrived: checks it, and stores it if
/// it's what it should be and something here refers to the blob.
pub(crate) async fn accept<V: View>(
    step: &mut Step<V>,
    blob: Listed,
    part: Part,
    bytes: Vec<u8>,
) -> Result<Accepted, StoreError> {
    let sized = length(blob, part) == Some(bytes.len() as u64);
    if !sized || !tally::is_referred(step, blob).await? {
        return Ok(Accepted::Dropped);
    }

    let (number, mut kept) = kept::ensure(step, blob).await?;
    if !checks(step, number, blob, part, &bytes).await? {
        return Ok(Accepted::Dropped);
    }

    match part {
        Part::Top => step.put(keys::top(number), bytes),
        Part::Leaves(page) => step.put(keys::leaf_page(number, page), bytes),
        Part::Piece(index) => {
            step.put(keys::piece(number, index), bytes);
            hold(&mut kept, groups_of(blob, index));
            step.save(keys::meta(number), &kept);
        }
    }

    Ok(if kept.is_whole() {
        Accepted::Whole
    } else {
        Accepted::Stored
    })
}

/// The parts of `blob` to ask for next: up to `room` of those that
/// aren't here and that `flying` doesn't say are on their way.
pub(crate) async fn missing<V: View>(
    step: &Step<V>,
    blob: Listed,
    room: usize,
    flying: impl Fn(Part) -> bool,
) -> Result<Vec<Part>, StoreError> {
    let groups = hash::groups(blob.length);
    let found = kept::find(step, blob).await?;
    let number = found.as_ref().map(|(number, _)| *number);
    let held = found.map_or_default(|(_, kept)| kept.held);
    let has = async |key: Vec<u8>| -> Result<bool, StoreError> {
        Ok(match number {
            Some(_) => step.get(&key).await?.is_some(),
            None => false,
        })
    };
    let number = number.unwrap_or_default();
    if groups > PAGE && !has(keys::top(number)).await? {
        return Ok(if flying(Part::Top) {
            Vec::new()
        } else {
            vec![Part::Top]
        });
    }

    // What isn't held, as the gaps between what is.
    let starts = held.iter().map(|&(_, end)| end);
    let ends = held.iter().map(|&(start, _)| start).chain([groups]);
    let gaps = [0].into_iter().chain(starts).zip(ends);
    let mut wanted = Vec::new();
    let mut paged = BTreeMap::new();
    for (start, end) in gaps.filter(|(start, end)| start < end) {
        for index in start / GROUPS..end.div_ceil(GROUPS) {
            if wanted.len() >= room {
                return Ok(wanted);
            }

            // A piece is asked for once the hashes it's checked
            // against are here.
            let (first, last) = groups_of(blob, index);
            let mut checkable = true;
            for page in first / PAGE..=(last - 1) / PAGE {
                if groups == 1 {
                    break;
                }
                if let Entry::Vacant(vacant) = paged.entry(page) {
                    vacant.insert(has(keys::leaf_page(number, page)).await?);
                }
                if !paged[&page] {
                    checkable = false;
                    let leaves = Part::Leaves(page);
                    if !flying(leaves) && !wanted.contains(&leaves) {
                        wanted.push(leaves);
                    }
                }
            }
            if !checkable {
                return Ok(wanted);
            }
            if !flying(Part::Piece(index)) {
                wanted.push(Part::Piece(index));
            }
        }
    }

    Ok(wanted)
}
