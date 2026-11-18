//! Where blobs are kept: beside the groups, and outside all of them.
//!
//! ```text
//! [BLOBS][REFS][group][blob]    → how many things the group holds that refer to the blob
//! [BLOBS][BY][blob][group]      → that the group refers to the blob
//! [BLOBS][NEXT]                 → the number the next blob kept here is given
//! [BLOBS][NAME][blob]           → the number of the blob that's kept
//! [BLOBS][META][number]         → what's kept of it
//! [BLOBS][DATA][number][index]  → one piece of its bytes
//! [BLOBS][LEAF][number][page]   → the hashes of its groups, a page at a time
//! [BLOBS][TOP][number]          → the hash of each full page
//! [BLOBS][IMPORT][number]       → an import: unfinished, or finished and not yet referred to
//! [BLOBS][DOOMED][number]       → that its bytes are being deleted
//! ```
//!
//! A blob is its name and its length, so that's what `[blob]` is. Its
//! bytes are filed under a number of the store's own, because an
//! import has bytes before it has a name.
//!
//! What refers to a blob is filed twice: by group, so that a group
//! takes its counts with it in one range, and by blob, so that
//! whether anything still refers to one is the start of a range.

#![cfg_attr(
    not(feature = "blobs"),
    expect(dead_code, reason = "bytes are only kept with the feature")
)]

use super::Listed;
use crate::db::GroupId;

const BLOBS: u8 = 5;

const REFS: u8 = 1;
const BY: u8 = 2;
const NEXT: u8 = 3;
const NAME: u8 = 4;
const META: u8 = 5;
const DATA: u8 = 6;
const LEAF: u8 = 7;
const TOP: u8 = 8;
const IMPORT: u8 = 9;
const DOOMED: u8 = 10;

fn blob(blob: Listed) -> Vec<u8> {
    [&blob.name[..], &blob.length.to_be_bytes()].concat()
}

/// How many things `group` holds that refer to `listed`.
pub(crate) fn refs(group: GroupId, listed: Listed) -> Vec<u8> {
    [&[BLOBS, REFS][..], group.as_bytes(), &blob(listed)].concat()
}

/// Every group that refers to `listed`.
pub(crate) fn referrers(listed: Listed) -> Vec<u8> {
    [&[BLOBS, BY][..], &blob(listed)].concat()
}

/// That `group` refers to `listed`.
pub(crate) fn referred_by(listed: Listed, group: GroupId) -> Vec<u8> {
    [referrers(listed), group.as_bytes().to_vec()].concat()
}

/// Whether anything was ever counted: every count there is.
pub(crate) fn counts() -> Vec<u8> {
    vec![BLOBS, REFS]
}

pub(crate) fn next() -> Vec<u8> {
    vec![BLOBS, NEXT]
}

pub(crate) fn name(listed: Listed) -> Vec<u8> {
    [&[BLOBS, NAME][..], &blob(listed)].concat()
}

pub(crate) fn meta(number: u64) -> Vec<u8> {
    [&[BLOBS, META][..], &number.to_be_bytes()].concat()
}

/// Every piece of a blob's bytes.
pub(crate) fn pieces(number: u64) -> Vec<u8> {
    [&[BLOBS, DATA][..], &number.to_be_bytes()].concat()
}

pub(crate) fn piece(number: u64, index: u64) -> Vec<u8> {
    [pieces(number), index.to_be_bytes().to_vec()].concat()
}

/// Every page of a blob's hashes.
pub(crate) fn leaves(number: u64) -> Vec<u8> {
    [&[BLOBS, LEAF][..], &number.to_be_bytes()].concat()
}

pub(crate) fn leaf_page(number: u64, page: u64) -> Vec<u8> {
    [leaves(number), page.to_be_bytes().to_vec()].concat()
}

pub(crate) fn top(number: u64) -> Vec<u8> {
    [&[BLOBS, TOP][..], &number.to_be_bytes()].concat()
}

pub(crate) fn imports() -> Vec<u8> {
    vec![BLOBS, IMPORT]
}

pub(crate) fn import(number: u64) -> Vec<u8> {
    [imports(), number.to_be_bytes().to_vec()].concat()
}

pub(crate) fn all_doomed() -> Vec<u8> {
    vec![BLOBS, DOOMED]
}

pub(crate) fn doomed(number: u64) -> Vec<u8> {
    [all_doomed(), number.to_be_bytes().to_vec()].concat()
}

/// The number at the end of a key that ends in one.
pub(crate) fn number_of(key: &[u8]) -> Option<u64> {
    key.last_chunk::<8>().map(|it| u64::from_be_bytes(*it))
}
