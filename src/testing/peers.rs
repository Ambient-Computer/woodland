//! The devices that appear in tests. Three belong to one person; the friend
//! is someone else, invited later.

use crate::actor::{Identity, Peer};

pub(crate) fn phone_identity() -> Identity {
    Identity::from_secret_key([1; 32])
}

pub(crate) fn laptop_identity() -> Identity {
    Identity::from_secret_key([2; 32])
}

pub(crate) fn nas_identity() -> Identity {
    Identity::from_secret_key([3; 32])
}

pub(crate) fn friend_identity() -> Identity {
    Identity::from_secret_key([4; 32])
}

pub(crate) fn phone() -> Peer {
    phone_identity().peer()
}

pub(crate) fn laptop() -> Peer {
    laptop_identity().peer()
}

pub(crate) fn nas() -> Peer {
    nas_identity().peer()
}

pub(crate) fn friend() -> Peer {
    friend_identity().peer()
}
