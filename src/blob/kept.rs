//! The bytes a store keeps, and how they get there and go.
//!
//! A blob's bytes are kept in pieces of a few groups each, under a
//! number the store gives it. Beside them are the hashes of its
//! groups, and of its pages of groups: see [`hash`](super::hash).
//!
//! Everything here does a bounded amount per step, however long the
//! blob is.

use blake3::hazmat::ChainingValue;
use serde::{Deserialize, Serialize};

use super::hash::{self, GROUP, PAGE};
use super::{BlobRef, Listed, keys, tally};
use crate::db::keys as held;
use crate::db::step::Step;
use crate::store::{StoreError, View};

/// How many groups are stored as one value. Seven is where both the
/// file and the browser's database did best.
pub(crate) const GROUPS: u64 = 7;

/// How many bytes are stored as one value.
pub(crate) const PIECE: u64 = GROUPS * GROUP;

/// The most bytes one step takes in, or hands back.
pub(crate) const MOST: usize = 9 * 112 * 1024;

/// How many pieces one step deletes.
const SWEPT: u64 = 32;

/// How many pages of hashes one step deletes.
const PAGES_SWEPT: usize = 16;

/// What's kept of a blob under its number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Kept {
    /// What it is. An import has no name until it ends.
    pub(crate) blob: Option<Listed>,
    /// The key it's encrypted under, if this device imported it for
    /// sealed groups.
    pub(crate) key: Option<[u8; 32]>,
    /// How many bytes an import has taken in so far.
    pub(crate) taken: u64,
    /// The groups whose bytes are held, as ranges in order that
    /// neither touch nor overlap.
    pub(crate) held: Vec<(u64, u64)>,
    /// Whether an import holds it: one that isn't finished, or one
    /// nothing refers to yet.
    pub(crate) import: bool,
}

impl Kept {
    /// Whether every byte is held.
    pub(crate) fn is_whole(&self) -> bool {
        let Some(blob) = self.blob else {
            return false;
        };

        match self.held.as_slice() {
            [] => blob.length == 0,
            [(0, end)] => *end == hash::groups(blob.length),
            _ => false,
        }
    }

    /// How many bytes are held.
    pub(crate) fn bytes(&self) -> u64 {
        let Some(blob) = self.blob else {
            return self.taken;
        };
        let size = |&(start, end): &(u64, u64)| (end * GROUP).min(blob.length) - start * GROUP;

        self.held.iter().map(size).sum()
    }

    /// Whether every group that `offset..end` touches is held.
    pub(crate) fn covers(&self, offset: u64, end: u64) -> bool {
        let (first, last) = (offset / GROUP, end.div_ceil(GROUP));
        let within = |&(start, end): &(u64, u64)| start <= first && last <= end;

        offset == end || self.held.iter().any(within)
    }

    /// The reference to it, once it has a name.
    pub(crate) fn reference(&self) -> Option<BlobRef> {
        let blob = self.blob?;

        Some(BlobRef::new(blob.name, blob.length, self.key))
    }
}

/// What a blob's bytes are being deleted up to.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Doomed {
    /// How many pieces may still be there.
    pieces: u64,
}

pub(crate) fn size(length: u64) -> usize {
    usize::try_from(length).expect("a piece is far smaller than memory")
}

pub(crate) async fn kept<V: View>(step: &Step<V>, number: u64) -> Result<Option<Kept>, StoreError> {
    step.load(&keys::meta(number)).await
}

/// The number the blob is kept under, and what's kept.
pub(crate) async fn find<V: View>(
    step: &Step<V>,
    blob: Listed,
) -> Result<Option<(u64, Kept)>, StoreError> {
    let Some(number) = step.load::<u64>(&keys::name(blob)).await? else {
        return Ok(None);
    };

    Ok(kept(step, number).await?.map(|kept| (number, kept)))
}

/// Whether `blob` is the one with no bytes, which every device holds
/// all of.
pub(crate) fn is_empty(blob: Listed) -> bool {
    blob.length == 0 && blob.name == hash::small(&[])
}

/// Whether every byte of `blob` is held.
pub(crate) async fn is_whole<V: View>(step: &Step<V>, blob: Listed) -> Result<bool, StoreError> {
    let found = find(step, blob).await?;

    Ok(is_empty(blob) || found.is_some_and(|(_, kept)| kept.is_whole()))
}

/// What's kept of `blob`, which something refers to: made if there
/// was nothing, and no longer going if it was.
pub(crate) async fn ensure<V: View>(
    step: &mut Step<V>,
    blob: Listed,
) -> Result<(u64, Kept), StoreError> {
    if let Some((number, kept)) = find(step, blob).await? {
        step.delete(keys::doomed(number));

        return Ok((number, kept));
    }

    let number = step.load::<u64>(&keys::next()).await?.unwrap_or_default();
    step.save(keys::next(), &(number + 1));
    let kept = Kept {
        blob: Some(blob),
        key: None,
        taken: 0,
        held: Vec::new(),
        import: false,
    };
    step.save(keys::meta(number), &kept);
    step.save(keys::name(blob), &number);

    Ok((number, kept))
}

/// Lets go of the bytes of `blob`, whatever refers to it. Whether
/// there were any that no import holds.
pub(crate) async fn release<V: View>(step: &mut Step<V>, blob: Listed) -> Result<bool, StoreError> {
    free(step, blob).await
}

/// Starts an import, and says the number it's kept under.
pub(crate) async fn begin<V: View>(
    step: &mut Step<V>,
    key: Option<[u8; 32]>,
) -> Result<u64, StoreError> {
    let number = step.load::<u64>(&keys::next()).await?.unwrap_or_default();
    step.save(keys::next(), &(number + 1));
    step.put(keys::import(number), Vec::new());
    let kept = Kept {
        blob: None,
        key,
        taken: 0,
        held: Vec::new(),
        import: true,
    };
    step.save(keys::meta(number), &kept);

    Ok(number)
}

/// The import kept under `number`, if it's still taking bytes in.
async fn importing<V: View>(step: &Step<V>, number: u64) -> Result<Option<Kept>, StoreError> {
    let kept = kept(step, number).await?;

    Ok(kept.filter(|kept| kept.import && kept.blob.is_none()))
}

pub(crate) async fn hashes<V: View>(
    step: &Step<V>,
    key: &[u8],
) -> Result<Vec<ChainingValue>, StoreError> {
    let stored = step.get(key).await?.unwrap_or_default();

    Ok(stored.as_chunks::<32>().0.to_vec())
}

/// Files the hash of the group at `index`, and of its page if that
/// fills it.
async fn file<V: View>(
    step: &mut Step<V>,
    number: u64,
    index: u64,
    hash: ChainingValue,
) -> Result<(), StoreError> {
    let page = keys::leaf_page(number, index / PAGE);
    let mut filed = hashes(step, &page).await?;
    filed.truncate(size(index % PAGE));
    filed.push(hash);
    if filed.len() == size(PAGE) {
        let mut top = step.get(&keys::top(number)).await?.unwrap_or_default();
        top.truncate(size(index / PAGE) * 32);
        top.extend(hash::subtree(&filed));
        step.put(keys::top(number), top);
    }
    step.put(page, filed.concat());

    Ok(())
}

/// Takes in the next bytes of an import: as many of `bytes` as one
/// step takes, and says how many that was. `None` if there's no such
/// import.
pub(crate) async fn append<V: View>(
    step: &mut Step<V>,
    number: u64,
    bytes: &[u8],
) -> Result<Option<usize>, StoreError> {
    let Some(mut kept) = importing(step, number).await? else {
        return Ok(None);
    };

    let bytes = &bytes[..bytes.len().min(MOST)];
    let mut at = kept.taken;
    let mut rest = bytes;
    while !rest.is_empty() {
        let (index, within) = (at / PIECE, size(at % PIECE));
        let key = keys::piece(number, index);
        let mut piece = match within {
            0 => Vec::new(),
            _ => step.get(&key).await?.unwrap_or_default(),
        };
        piece.truncate(within);
        let room = size(PIECE) - piece.len();
        let (taken, after) = rest.split_at(rest.len().min(room));
        piece.extend_from_slice(taken);

        // Every group this fills is hashed. The last one may yet be
        // the whole blob, and is hashed when the import ends.
        let before = at / GROUP;
        at += taken.len() as u64;
        for group in before..at / GROUP {
            let start = size(group * GROUP - index * PIECE);
            let filled = &piece[start..start + size(GROUP)];
            file(step, number, group, hash::group(group * GROUP, filled)).await?;
        }

        step.put(key, piece);
        rest = after;
    }

    kept.taken = at;
    step.save(keys::meta(number), &kept);

    Ok(Some(bytes.len()))
}

/// The name of the import kept under `number`, which is `length`
/// bytes and has every group's hash filed.
async fn name<V: View>(step: &Step<V>, number: u64, length: u64) -> Result<[u8; 32], StoreError> {
    let groups = hash::groups(length);
    if groups <= 1 {
        let only = step.get(&keys::piece(number, 0)).await?;

        return Ok(hash::small(&only.unwrap_or_default()));
    }
    if groups <= PAGE {
        return Ok(hash::root(
            &hashes(step, &keys::leaf_page(number, 0)).await?,
        ));
    }

    Ok(hash::root(&hashes(step, &keys::top(number)).await?))
}

/// Ends an import: its bytes are a blob, and this is the reference to
/// it. `None` if there's no such import.
///
/// If the store already holds all of that blob, it keeps the one
/// copy it had.
pub(crate) async fn finish<V: View>(
    step: &mut Step<V>,
    number: u64,
) -> Result<Option<BlobRef>, StoreError> {
    let Some(mut kept) = importing(step, number).await? else {
        return Ok(None);
    };

    let length = kept.taken;
    let groups = hash::groups(length);
    if !length.is_multiple_of(GROUP) {
        let start = (groups - 1) * GROUP;
        let piece = step.get(&keys::piece(number, start / PIECE)).await?;
        let piece = piece.unwrap_or_default();
        let last = &piece[size(start % PIECE)..];
        file(step, number, groups - 1, hash::group(start, last)).await?;
    }

    // The last page's hash is filed with the others, full or not, so
    // that the pages' hashes are all another device needs to start.
    if groups > PAGE && !groups.is_multiple_of(PAGE) {
        let last = hashes(step, &keys::leaf_page(number, groups / PAGE)).await?;
        let mut top = step.get(&keys::top(number)).await?.unwrap_or_default();
        top.truncate(size(groups / PAGE) * 32);
        top.extend(hash::subtree(&last));
        step.put(keys::top(number), top);
    }

    let blob = Listed {
        name: name(step, number, length).await?,
        length,
    };
    let referred = tally::is_referred(step, blob).await?;
    let reference = BlobRef::new(blob.name, blob.length, kept.key);
    match find(step, blob).await? {
        Some((other, mut whole)) if whole.is_whole() => {
            // It's here already. The import holds that copy now.
            step.delete(keys::import(number));
            kept.import = false;
            step.save(keys::meta(number), &kept);
            doom(step, number, &kept);
            whole.import = !referred;
            step.save(keys::meta(other), &whole);
            step.delete(keys::doomed(other));
            if !referred {
                step.put(keys::import(other), Vec::new());
            }

            return Ok(Some(reference));
        }
        Some((other, part)) => doom(step, other, &part),
        None => {}
    }

    kept.blob = Some(blob);
    kept.held = match groups {
        0 => Vec::new(),
        _ => vec![(0, groups)],
    };
    kept.import = !referred;
    if referred {
        step.delete(keys::import(number));
    }
    step.save(keys::meta(number), &kept);
    step.save(keys::name(blob), &number);

    Ok(Some(reference))
}

/// Lets an import go: one that's unfinished, or one nothing refers
/// to. Whether there was one.
pub(crate) async fn abandon<V: View>(step: &mut Step<V>, number: u64) -> Result<bool, StoreError> {
    let Some(mut kept) = kept(step, number).await?.filter(|kept| kept.import) else {
        return Ok(false);
    };

    step.delete(keys::import(number));
    kept.import = false;
    step.save(keys::meta(number), &kept);
    let referred = match kept.blob {
        Some(blob) => tally::is_referred(step, blob).await?,
        None => false,
    };
    if !referred {
        doom(step, number, &kept);
    }

    Ok(true)
}

/// Every import there is: the number each is kept under, and what's
/// kept.
pub(crate) async fn imports<V: View>(step: &Step<V>) -> Result<Vec<(u64, Kept)>, StoreError> {
    let (start, end) = held::range(keys::imports());
    let mut found = Vec::new();
    for (key, _) in step.scan(&start, &end).await? {
        let Some(number) = keys::number_of(&key) else {
            continue;
        };
        if let Some(kept) = kept(step, number).await? {
            found.push((number, kept));
        }
    }

    Ok(found)
}

/// Marks a blob's bytes to be deleted, a step at a time.
fn doom<V: View>(step: &mut Step<V>, number: u64, kept: &Kept) {
    let reach = kept.held.last().map_or(0, |&(_, end)| end * GROUP);
    let doomed = Doomed {
        pieces: reach.max(kept.taken).div_ceil(PIECE),
    };
    step.save(keys::doomed(number), &doomed);
}

/// Nothing refers to `blob` any longer: its bytes go, unless an
/// import holds them.
pub(crate) async fn free<V: View>(step: &mut Step<V>, blob: Listed) -> Result<bool, StoreError> {
    match find(step, blob).await? {
        Some((number, kept)) if !kept.import => {
            doom(step, number, &kept);

            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Something refers to `blob` that nothing did. Whatever is left of
/// it stays, and it's no import's any longer.
pub(crate) async fn revive<V: View>(step: &mut Step<V>, blob: Listed) -> Result<(), StoreError> {
    let Some((number, mut kept)) = find(step, blob).await? else {
        return Ok(());
    };

    if kept.import {
        kept.import = false;
        step.delete(keys::import(number));
        step.save(keys::meta(number), &kept);
    }

    // Once its bytes are gone its hashes are going, and what's left
    // is no use to anyone.
    if !kept.held.is_empty() || blob.length == 0 {
        step.delete(keys::doomed(number));
    }

    Ok(())
}

/// Whether anything is being deleted.
pub(crate) async fn is_sweeping<V: View>(step: &Step<V>) -> Result<bool, StoreError> {
    let (start, end) = held::range(keys::all_doomed());

    Ok(!step.scan_page(&start, &end, 1).await?.is_empty())
}

/// Deletes one step's worth of what's to be deleted. Says which blob
/// lost bytes, if it was one with a name.
pub(crate) async fn sweep<V: View>(step: &mut Step<V>) -> Result<Option<Listed>, StoreError> {
    let (start, end) = held::range(keys::all_doomed());
    let Some((key, doomed)) = step.scan_page(&start, &end, 1).await?.pop() else {
        return Ok(None);
    };
    let Some(number) = keys::number_of(&key) else {
        step.delete(key);

        return Ok(None);
    };
    let kept = kept(step, number).await?;
    let doomed = crate::encoding::decode::<Doomed>(&doomed);
    let (Some(mut kept), Some(mut doomed)) = (kept, doomed) else {
        step.delete(key);

        return Ok(None);
    };

    if doomed.pieces > 0 {
        let left = doomed.pieces.saturating_sub(SWEPT);
        for index in left..doomed.pieces {
            step.delete(keys::piece(number, index));
        }

        doomed.pieces = left;
        let groups = left * GROUPS;
        kept.held.retain(|&(start, _)| start < groups);
        if let Some(last) = kept.held.last_mut() {
            last.1 = last.1.min(groups);
        }
        kept.taken = kept.taken.min(left * PIECE);
        step.save(keys::meta(number), &kept);
        step.save(key, &doomed);

        return Ok(kept.blob);
    }

    let (start, end) = held::range(keys::leaves(number));
    let pages = step.scan_page(&start, &end, PAGES_SWEPT).await?;
    let more = pages.len() == PAGES_SWEPT;
    for (page, _) in pages {
        step.delete(page);
    }
    if more {
        return Ok(None);
    }

    step.delete(keys::top(number));
    step.delete(keys::meta(number));
    step.delete(keys::import(number));
    step.delete(key);
    if let Some(blob) = kept.blob
        && step.load::<u64>(&keys::name(blob)).await? == Some(number)
    {
        step.delete(keys::name(blob));
    }

    Ok(kept.blob)
}

/// What a read of a blob's bytes found.
pub(crate) enum Bytes {
    Held(Vec<u8>),
    NotHeld,
}

/// Up to `length` of `blob`'s bytes from `offset`, as they're kept:
/// fewer at the blob's end, or if it's more than one step hands back.
pub(crate) async fn read<V: View>(
    step: &Step<V>,
    blob: Listed,
    offset: u64,
    length: usize,
) -> Result<Bytes, StoreError> {
    if is_empty(blob) {
        return Ok(Bytes::Held(Vec::new()));
    }
    let Some((number, kept)) = find(step, blob).await? else {
        return Ok(Bytes::NotHeld);
    };

    let offset = offset.min(blob.length);
    let end = offset.saturating_add(length.min(MOST) as u64);
    let end = end.min(blob.length);
    if !kept.covers(offset, end) {
        return Ok(Bytes::NotHeld);
    }

    let mut found = Vec::with_capacity(size(end - offset));
    let mut at = offset;
    while at < end {
        let index = at / PIECE;
        let Some(piece) = step.get(&keys::piece(number, index)).await? else {
            return Ok(Bytes::NotHeld);
        };
        let from = size(at - index * PIECE);
        let to = size((end - index * PIECE).min(PIECE)).min(piece.len());
        if to <= from {
            return Ok(Bytes::NotHeld);
        }

        found.extend_from_slice(&piece[from..to]);
        at += (to - from) as u64;
    }

    Ok(Bytes::Held(found))
}
