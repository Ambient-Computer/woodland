//! What names a model, and the kind of model it names.

use serde::{Deserialize, Serialize};

use super::{Extension, extension, vocabulary};
use crate::db::ClockId;
use crate::encoding;

/// Set in the first byte of the ID of a model that's restricted to
/// committed reads.
const HELD: u8 = 0x80;

/// The kinds of model there are. A model's kind is part of its ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Kv = 1,
    Claims = 2,
    Threshold = 3,
    Table = 4,
    Allowances = 5,
    Sequence = 6,
}

impl Kind {
    /// Whether readers of this kind see writes that aren't committed,
    /// unless the model says otherwise.
    fn shows_uncommitted(self) -> bool {
        !matches!(self, Self::Threshold)
    }

    /// Whether a key holds every write to it combined, rather than
    /// the last.
    pub(crate) fn merges(self) -> bool {
        matches!(self, Self::Sequence)
    }
}

/// Names a model: the clock that owns it, then its kind and the BLAKE3
/// hash of the clock's ID and a nonce.
///
/// Like a group ID, it's derived from how the model began, so models
/// created independently on different devices never collide. The model's
/// initial state is always empty, so the nonce is the only input that varies.
///
/// A group's first clock is named after the group, so a model can be
/// made from a [`GroupId`] alone.
///
/// [`GroupId`]: crate::GroupId
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ModelId {
    clock: ClockId,
    id: [u8; 32],
}

impl ModelId {
    /// A key-value map: the last write to a key wins. The application
    /// supplies the nonce: random bytes.
    #[must_use]
    pub fn kv(clock: impl Into<ClockId>, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Kv, clock.into(), nonce)
    }

    /// A map of claims: the first claim on a key wins, and a transaction
    /// that loses one doesn't happen at all.
    #[must_use]
    pub fn claims(clock: impl Into<ClockId>, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Claims, clock.into(), nonce)
    }

    /// A key-value map that only shows what is committed: values every
    /// contributor holds, in an order that's final. Reads of it never go
    /// back. Written like a [key-value] map.
    ///
    /// [key-value]: ModelId::kv
    #[must_use]
    pub fn threshold(clock: impl Into<ClockId>, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Threshold, clock.into(), nonce)
    }

    /// Allowances: quantities that several devices spend from, each
    /// out of its own share, so that none is ever overspent. See
    /// [`Transaction::open_allowance`].
    ///
    /// [`Transaction::open_allowance`]: crate::Transaction::open_allowance
    #[must_use]
    pub fn allowances(clock: impl Into<ClockId>, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Allowances, clock.into(), nonce)
    }

    /// Sequences: ordered lists that merge. What two devices put into
    /// one list while apart is all there when they meet, where each
    /// put it. See [`Transaction::insert_at`].
    ///
    /// [`Transaction::insert_at`]: crate::Transaction::insert_at
    #[must_use]
    pub fn sequence(clock: impl Into<ClockId>, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Sequence, clock.into(), nonce)
    }

    /// A model of a kind the application added with [`extend`], by the
    /// number it was added under. `None` if the number is 64 or more.
    ///
    /// [`extend`]: crate::extend
    #[must_use]
    pub fn of_kind(number: u8, clock: impl Into<ClockId>, nonce: [u8; 32]) -> Option<Self> {
        let clock = clock.into();
        let mut id = encoding::hash("woodland/model", &(clock.as_bytes(), nonce));
        id[0] = extension::FIRST
            .checked_add(number)
            .filter(|it| it & HELD == 0)?;

        Some(Self { clock, id })
    }

    /// A table: rows of typed cells, in columns that are themselves
    /// part of the data. It exists once a transaction [creates] it.
    ///
    /// [creates]: crate::Transaction::create_table
    #[must_use]
    pub fn table(clock: impl Into<ClockId>, nonce: [u8; 32]) -> Self {
        Self::new(Kind::Table, clock.into(), nonce)
    }

    /// The same kind of model, restricted to committed reads: nobody
    /// ever reads a value from it that could still be displaced, its
    /// own author included. It's a different model from the one this
    /// started as, with its own keys.
    #[must_use]
    pub fn committed_only(mut self) -> Self {
        self.id[0] |= HELD;
        self
    }

    /// Whether readers see writes that aren't committed. If not, they
    /// aren't kept as versions at all: they wait in the log.
    pub(crate) fn shows_uncommitted(&self) -> bool {
        let shows = self
            .kind()
            .map_or(self.extension().is_some(), Kind::shows_uncommitted);

        self.id[0] & HELD == 0 && shows
    }

    /// `None` for an ID this version of the library didn't make.
    pub(crate) fn kind(&self) -> Option<Kind> {
        [
            Kind::Kv,
            Kind::Claims,
            Kind::Threshold,
            Kind::Table,
            Kind::Allowances,
            Kind::Sequence,
        ]
        .into_iter()
        .find(|&kind| kind as u8 == self.id[0] & !HELD && kind as u8 <= vocabulary())
    }

    /// The kind an application added that the model is of, if this
    /// device has it.
    pub(crate) fn extension(&self) -> Option<Extension> {
        extension::of(self.id[0] & !HELD)
    }

    /// Whether a key of the model holds every write to it combined,
    /// rather than the last.
    pub(crate) fn merges(&self) -> bool {
        let extended = self.extension().is_some_and(|kind| kind.combine.is_some());

        self.kind().is_some_and(Kind::merges) || extended
    }

    /// Whether the model is of a kind this device has.
    pub(crate) fn is_known(&self) -> bool {
        self.kind().is_some() || self.extension().is_some()
    }

    /// The clock the model belongs to. A transaction writes to the
    /// models of one clock.
    #[must_use]
    pub fn clock(&self) -> ClockId {
        self.clock
    }

    fn new(kind: Kind, clock: ClockId, nonce: [u8; 32]) -> Self {
        let mut id = encoding::hash("woodland/model", &(clock.as_bytes(), nonce));
        id[0] = kind as u8;

        Self { clock, id }
    }

    /// The ID as bytes, to keep or send. The clock, then the model
    /// within it: a clock's models are one range of keys.
    #[must_use]
    pub fn as_bytes(&self) -> [u8; 64] {
        let mut bytes = [0; 64];
        bytes[..32].copy_from_slice(self.clock.as_bytes());
        bytes[32..].copy_from_slice(&self.id);

        bytes
    }

    /// The model that [`as_bytes`] gave these for. The kind is part
    /// of the ID, so bytes from anywhere else name a model of a kind
    /// nothing has.
    ///
    /// [`as_bytes`]: Self::as_bytes
    #[must_use]
    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        let mut clock = [0; 32];
        let mut id = [0; 32];
        clock.copy_from_slice(&bytes[..32]);
        id.copy_from_slice(&bytes[32..]);

        Self {
            clock: ClockId::from_bytes(clock),
            id,
        }
    }
}

/// Abbreviated, like a git hash.
impl std::fmt::Debug for ModelId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in &self.id[..4] {
            write!(formatter, "{byte:02x}")?;
        }

        Ok(())
    }
}
