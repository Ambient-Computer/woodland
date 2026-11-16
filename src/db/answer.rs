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
    /// Only what's committed. It's the same on every device that has
    /// got this far, and never goes back.
    Committed(Vec<Found>),
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
        matches!(self, Self::Committed(_))
    }

    /// What was found, whichever it is.
    #[must_use]
    pub fn found(&self) -> &[Found] {
        match self {
            Self::Optimistic(found) | Self::Committed(found) => found,
        }
    }

    /// What was found, whichever it is.
    #[must_use]
    pub fn into_found(self) -> Vec<Found> {
        match self {
            Self::Optimistic(found) | Self::Committed(found) => found,
        }
    }

    /// What was found, if it's committed.
    #[must_use]
    pub fn committed(self) -> Option<Vec<Found>> {
        match self {
            Self::Committed(found) => Some(found),
            Self::Optimistic(_) => None,
        }
    }
}
