//! Clock utilities: how much of each contributor's history a device holds,
//! and how writes are ordered.
//!
//! - [`VersionVector`]: per author, how many of their transactions this
//!   device has integrated. Authors' transactions are integrated in order,
//!   so "3" means "transactions 1 through 3".
//! - [`Stamp`]: a Lamport timestamp plus author, giving every transaction a
//!   total order for last-writer-wins.
//! - [`Progress`]: every contributor's self-reported version vector, and the
//!   greatest lower bound across them: the point every contributor has
//!   reached, below which history can be compacted.
//!
//! None of this knows about agreements or models. The agreement carries
//! [`Progress`] as the value it agrees on, and models stamp their
//! operations, but the clock math stays here.

mod progress;
mod stamp;
mod version_vector;

pub(crate) use progress::Progress;
pub(crate) use stamp::Stamp;
pub(crate) use version_vector::VersionVector;
