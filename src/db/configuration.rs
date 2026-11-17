//! Who's in a group, as this device knows it.

use ed25519_dalek::VerifyingKey;

use crate::actor::Peer;
use crate::chain::{Electorate, Role};
use crate::store::Store;

use super::{Database, GroupId};

/// A group's configuration: who holds a role in it, and what each may
/// do. [`Database::configuration`] gives the one in force.
///
/// It's what the latest checkpoint this device holds says, which is
/// all a device ever goes by. Another device may hold a later one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Configuration {
    /// The height of the checkpoint it's from.
    pub height: u64,
    /// Whether the group is sealed: only contributors and readers
    /// read it, and each needs a [sealing key].
    ///
    /// [sealing key]: Role::sealing_key
    pub sealed: bool,
    /// Everyone with a role. A peer that isn't here holds none: it can
    /// carry the group, and that's all.
    pub roles: Vec<(VerifyingKey, Role)>,
    /// What this device has [asked] the group to be made up of
    /// instead, while that still stands.
    ///
    /// [asked]: Database::reconfigure
    pub asked: Option<Vec<(VerifyingKey, Role)>>,
}

impl Configuration {
    /// The role `peer` holds, if it holds one.
    #[must_use]
    pub fn role(&self, peer: &VerifyingKey) -> Option<&Role> {
        let held = self.roles.iter().find(|(holder, _)| holder == peer);

        held.map(|(_, role)| role)
    }
}

/// What became of a request to [reconfigure] a group.
///
/// [reconfigure]: Database::reconfigure
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reconfigured {
    /// A checkpoint made the group what was asked for.
    Carried,
    /// The group changed some other way first, and the request did
    /// nothing. It named everyone, as the group was then, so it isn't
    /// asked again by itself: whoever the group gained meanwhile would
    /// be removed. Read the [configuration] and ask again.
    ///
    /// [configuration]: Database::configuration
    Spent,
}

fn roles(electorate: &Electorate) -> Vec<(VerifyingKey, Role)> {
    let named = |(peer, role): (Peer, &Role)| (peer.id(), role.clone());

    electorate.roles().map(named).collect()
}

impl<S: Store> Database<S> {
    /// Who's in `group` and what each may do, by the latest checkpoint
    /// this device holds. `None` if it doesn't hold the group.
    ///
    /// [`Changed::Members`] says when to read it again.
    ///
    /// [`Changed::Members`]: super::Changed::Members
    #[must_use]
    pub fn configuration(&self, group: GroupId) -> Option<Configuration> {
        let group = self.groups.get(&group)?;

        Some(Configuration {
            height: group.height(),
            sealed: group.is_sealed(),
            roles: roles(group.electorate()),
            asked: group.asked().map(roles),
        })
    }
}
