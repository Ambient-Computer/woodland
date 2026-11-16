//! What a read found, and how sure it is.

use crate::model::Query;

/// A key and what it holds.
pub type Found = (Vec<u8>, Vec<u8>);

/// The answer to a [query]: what it found, by how certain that is.
/// The two can't be told apart by what's in them, so they're told
/// apart here, and nothing that needs the one is handed the other
/// without saying so.
///
/// [query]: crate::Query
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Everything this device knows of, committed or not. It's the
    /// latest there is, and may be revised.
    Optimistic(Vec<Found>),
    /// Only what's committed, and how far that has got. It's the same
    /// on every device that has got this far, and never goes back.
    Committed(Vec<Found>, Reached),
}

/// How far what's committed on a device has got: which checkpoint a
/// committed read is of.
///
/// A committed read is the state of exactly one checkpoint. One that
/// takes a device many steps to put in place shows all at once, when
/// the last of it is there.
///
/// A copy kept somewhere else of what's committed, kept up from each
/// step's [`Changed`], is the state of a checkpoint whenever a step
/// reports [`Changed::Checkpoint`]. A copy that was last at one, and
/// finds the device at the same one, has missed nothing.
///
/// [`Changed`]: crate::Changed
/// [`Changed::Checkpoint`]: crate::Changed::Checkpoint
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reached {
    /// The height of the checkpoint.
    pub checkpoint: u64,
}

/// Some of the answer to a [query], and how to ask for the rest.
///
/// [query]: crate::Query
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// What was found, up to the limit asked for.
    pub answer: Answer,
    /// The query for what comes after, if the limit was what stopped
    /// this one. It may find nothing.
    pub rest: Option<Query>,
}

impl Answer {
    /// Whether this is only what's committed.
    #[must_use]
    pub fn is_committed(&self) -> bool {
        matches!(self, Self::Committed(..))
    }

    /// What was found, whichever it is.
    #[must_use]
    pub fn found(&self) -> &[Found] {
        match self {
            Self::Optimistic(found) | Self::Committed(found, _) => found,
        }
    }

    /// How far what's committed had got, if this is only what's
    /// committed.
    #[must_use]
    pub fn reached(&self) -> Option<Reached> {
        match self {
            Self::Optimistic(_) => None,
            Self::Committed(_, reached) => Some(*reached),
        }
    }

    /// What was found, whichever it is.
    #[must_use]
    pub fn into_found(self) -> Vec<Found> {
        match self {
            Self::Optimistic(found) | Self::Committed(found, _) => found,
        }
    }

    /// What was found, if it's committed.
    #[must_use]
    pub fn committed(self) -> Option<Vec<Found>> {
        match self {
            Self::Committed(found, _) => Some(found),
            Self::Optimistic(_) => None,
        }
    }
}
