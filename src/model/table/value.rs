use serde::{Deserialize, Serialize};

use crate::blob::BlobRef;
use crate::encoding;
use crate::model::ModelId;

/// What a cell holds.
///
/// There's nothing here that has to be worked out when a transaction is
/// applied: no "now", no "next ID", no random number. Whoever writes
/// the transaction works those out and writes down the result, so every
/// device applies the same thing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Value {
    /// No value. Any column takes it.
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Boolean(bool),
    Bytes(Vec<u8>),
    /// A reference to a blob. The bytes are held apart, by whichever
    /// devices choose to.
    Blob(BlobRef),
}

/// The types of value a column can take. A column's type is part of
/// its ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Type {
    Integer = 1,
    Real = 2,
    Text = 3,
    Boolean = 4,
    Bytes = 5,
    Blob = 6,
}

/// Names a column of a table: its type, then a hash of the table's ID
/// and a nonce.
///
/// Like a [`ModelId`], it's derived from how the column began, so
/// columns added independently on different devices never collide, and a
/// column that's dropped and added again is a different column. The
/// name people see is kept in the table, and can change.
///
/// The type can't change. That's what lets a value be checked against
/// its column without reading anything.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ColumnId([u8; 8]);

/// Set in the first byte of the ID of a unique column.
const UNIQUE: u8 = 0x80;

impl Value {
    fn kind(&self) -> Option<Type> {
        match self {
            Self::Null => None,
            Self::Integer(_) => Some(Type::Integer),
            Self::Real(_) => Some(Type::Real),
            Self::Text(_) => Some(Type::Text),
            Self::Boolean(_) => Some(Type::Boolean),
            Self::Bytes(_) => Some(Type::Bytes),
            Self::Blob(_) => Some(Type::Blob),
        }
    }
}

/// Two reals are the same value if they're the same bits, so that
/// equality is total and a `NaN` is equal to itself.
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Integer(ours), Self::Integer(theirs)) => ours == theirs,
            (Self::Real(ours), Self::Real(theirs)) => ours.to_bits() == theirs.to_bits(),
            (Self::Text(ours), Self::Text(theirs)) => ours == theirs,
            (Self::Boolean(ours), Self::Boolean(theirs)) => ours == theirs,
            (Self::Bytes(ours), Self::Bytes(theirs)) => ours == theirs,
            (Self::Blob(ours), Self::Blob(theirs)) => ours == theirs,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Real(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<BlobRef> for Value {
    fn from(value: BlobRef) -> Self {
        Self::Blob(value)
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

/// One column is a list of one, wherever a list of columns is asked
/// for.
impl AsRef<[Self]> for ColumnId {
    fn as_ref(&self) -> &[Self] {
        std::slice::from_ref(self)
    }
}

impl ColumnId {
    /// A column of whole numbers. The application supplies the nonce, as
    /// with a [`ModelId`].
    #[must_use]
    pub fn integer(table: ModelId, nonce: [u8; 32]) -> Self {
        Self::new(Type::Integer, table, nonce)
    }

    #[must_use]
    pub fn real(table: ModelId, nonce: [u8; 32]) -> Self {
        Self::new(Type::Real, table, nonce)
    }

    #[must_use]
    pub fn text(table: ModelId, nonce: [u8; 32]) -> Self {
        Self::new(Type::Text, table, nonce)
    }

    #[must_use]
    pub fn boolean(table: ModelId, nonce: [u8; 32]) -> Self {
        Self::new(Type::Boolean, table, nonce)
    }

    #[must_use]
    pub fn bytes(table: ModelId, nonce: [u8; 32]) -> Self {
        Self::new(Type::Bytes, table, nonce)
    }

    /// A column of references to blobs. It can't be unique: a blob
    /// says nothing about itself to tell two apart by.
    #[must_use]
    pub fn blob(table: ModelId, nonce: [u8; 32]) -> Self {
        Self::new(Type::Blob, table, nonce)
    }

    /// The same column, but unique: no two rows can hold the same value
    /// in it, other than none. Like the type, that's part of the ID and
    /// can't change, so decide before the column is added.
    #[must_use]
    pub fn unique(mut self) -> Self {
        self.0[0] |= UNIQUE;
        self
    }

    /// Whether its cells are references to blobs.
    pub(crate) fn holds_blobs(self) -> bool {
        self.kind() == Some(Type::Blob)
    }

    pub(crate) fn is_unique(self) -> bool {
        self.0[0] & UNIQUE != 0
    }

    /// Whether the column takes `value`: one of its type, or none.
    pub(crate) fn takes(self, value: &Value) -> bool {
        let kind = self.kind();
        let keyed = kind == Some(Type::Blob) && self.is_unique();

        kind.is_some() && !keyed && value.kind().is_none_or(|it| Some(it) == kind)
    }

    /// `None` for an ID this version of the library didn't make.
    pub(super) fn kind(self) -> Option<Type> {
        let types = [
            Type::Integer,
            Type::Real,
            Type::Text,
            Type::Boolean,
            Type::Bytes,
            Type::Blob,
        ];

        types
            .into_iter()
            .find(|&kind| kind as u8 == self.0[0] & !UNIQUE)
    }

    fn new(kind: Type, table: ModelId, nonce: [u8; 32]) -> Self {
        let hash = encoding::hash("woodland/column", &(table, nonce));
        let mut id = [0; 8];
        id.copy_from_slice(&hash[..8]);
        id[0] = kind as u8;

        Self(id)
    }

    /// The ID as bytes, to keep or send.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 8] {
        &self.0
    }

    /// The column that [`as_bytes`] gave these for. Its type, and
    /// whether it's unique, are part of the ID.
    ///
    /// [`as_bytes`]: Self::as_bytes
    #[must_use]
    pub fn from_bytes(bytes: [u8; 8]) -> Self {
        Self(bytes)
    }
}

impl std::fmt::Debug for ColumnId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in &self.0 {
            write!(formatter, "{byte:02x}")?;
        }

        Ok(())
    }
}
