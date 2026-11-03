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

/// The identity behind a cast member's peer.
pub(crate) fn identity_of(peer: Peer) -> Identity {
    [
        phone_identity(),
        laptop_identity(),
        nas_identity(),
        friend_identity(),
    ]
    .into_iter()
    .find(|identity| identity.peer() == peer)
    .expect("not in the cast")
}

/// The cast member's name, for readable debug output.
pub(crate) fn name(peer: Peer) -> Option<&'static str> {
    [
        (phone(), "phone"),
        (laptop(), "laptop"),
        (nas(), "nas"),
        (friend(), "friend"),
    ]
    .into_iter()
    .find_map(|(candidate, name)| (candidate == peer).then_some(name))
}
