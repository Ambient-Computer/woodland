//! Kinds of model an application adds.
//!
//! A kind is native code: every device in a group that uses it has to
//! have it, and has to have the same one. What it says about an
//! operation is held to the same rule as the rest of the core: the same
//! bytes in, the same answer out, on every device, for good.
//!
//! All a kind does is declare. Given an operation, as bytes that mean
//! something only to it, it says what the operation requires and what
//! it writes. Who wins, what is pruned, what is final and what is
//! shed follow from that, as they do for the kinds the library has.

use std::collections::BTreeMap;

use super::{Fingerprint, Requirement};

/// Where the numbers of applications' kinds start, in the first byte
/// of a model's ID. The library's own count up from one.
pub(super) const FIRST: u8 = 0x40;

/// How many kinds applications can add.
const KINDS: u8 = 0x40;

/// A kind of model that an application adds with [`extend`].
#[derive(Clone, Copy, Debug)]
pub struct Extension {
    /// Which kind it is, to every device that has it: under 64, and
    /// the application's to choose and never change.
    pub number: u8,
    /// What an operation requires and writes. `None` if it isn't one
    /// of this kind's: its transaction is pruned.
    pub declare: fn(&[u8]) -> Option<Declared>,
    /// For a kind that merges: what a key holds once a write is
    /// combined with what it held, if anything. With it, writes to one
    /// key by devices that hadn't heard of each other all count, in
    /// the timeline's order, where otherwise the last one would stand.
    /// A write of nothing still clears the key.
    ///
    /// It's held to the same rule as `declare`, and can't fail: a
    /// write that makes no sense leaves what was held.
    pub combine: Option<Combine>,
}

/// What a key holds once a write is combined with what it held.
pub type Combine = fn(Option<&[u8]>, &[u8]) -> Vec<u8>;

/// What one operation of an application's kind does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Declared {
    /// What has to be true of each key, at the transaction's place in
    /// the timeline, for the transaction to happen.
    pub requires: Vec<(Vec<u8>, Needs)>,
    /// What each key holds afterwards, in order: a value, or nothing.
    pub writes: Vec<(Vec<u8>, Option<Vec<u8>>)>,
}

/// What an operation needs to be true of a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Needs {
    /// It holds nothing.
    Vacant,
    /// It holds something.
    Present,
    /// It holds exactly this.
    Is(Vec<u8>),
}

impl From<Needs> for Requirement {
    fn from(needs: Needs) -> Self {
        match needs {
            Needs::Vacant => Self::Vacant,
            Needs::Present => Self::Present,
            Needs::Is(value) => Self::Is(Fingerprint::of(&value)),
        }
    }
}

#[cfg(not(test))]
static EXTENSIONS: std::sync::RwLock<BTreeMap<u8, Extension>> =
    std::sync::RwLock::new(BTreeMap::new());

#[cfg(test)]
thread_local! {
    static EXTENSIONS: std::cell::RefCell<BTreeMap<u8, Extension>> =
        const { std::cell::RefCell::new(BTreeMap::new()) };
}

/// Adds a kind of model, for every [`Database`] in the process. Do it
/// before opening one: a device that meets a kind it lacks stops
/// reading the clock it's in, and catches up the next time it's opened
/// with the kind in place.
///
/// Returns whether it was added. It isn't if the number is 64 or more,
/// or is taken.
///
/// [`Database`]: crate::Database
#[must_use]
pub fn extend(extension: Extension) -> bool {
    if extension.number >= KINDS {
        return false;
    }

    #[cfg(not(test))]
    let added = {
        let mut held = EXTENSIONS
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        held.insert(extension.number, extension).is_none()
    };
    #[cfg(test)]
    let added =
        EXTENSIONS.with_borrow_mut(|held| held.insert(extension.number, extension).is_none());

    added
}

#[cfg(test)]
thread_local! {
    static HIDDEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Makes this thread run as an application that has added no kinds,
/// or as the one it is.
#[cfg(test)]
pub(crate) fn hide(hidden: bool) {
    HIDDEN.set(hidden);
}

/// Every kind that's been added, by number.
fn all() -> BTreeMap<u8, Extension> {
    #[cfg(not(test))]
    let held = EXTENSIONS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    // A device running as an earlier version has none of them.
    #[cfg(test)]
    let held = if super::vocabulary() < super::LATEST || HIDDEN.get() {
        BTreeMap::new()
    } else {
        EXTENSIONS.with_borrow(Clone::clone)
    };

    held
}

/// The numbers of the kinds this device has.
pub(crate) fn numbers() -> std::collections::BTreeSet<u8> {
    all().into_keys().collect()
}

/// The kind a model is of, going by the first byte of its ID, if
/// it's an application's and this device has it.
pub(super) fn of(first: u8) -> Option<Extension> {
    let number = first.checked_sub(FIRST)?;

    all().remove(&number)
}
