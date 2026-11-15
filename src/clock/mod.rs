//! How entries are ordered.
//!
//! A [`Stamp`] is a Lamport timestamp and an author. It's greater than
//! the stamp of anything its entry was written on top of, and no two
//! entries share one, so stamps put any set of entries in one order that
//! every device agrees on.

mod stamp;

pub(crate) use stamp::Stamp;
