//! Who's in a group, as this device knows it.

use std::collections::BTreeSet;

use ed25519_dalek::VerifyingKey;

use crate::actor::Peer;
use crate::chain::{Electorate, Role};
use crate::store::Store;

use super::group::Group;
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

/// Where this device stands in a group it holds, by the latest
/// checkpoint it holds. [`Database::standing`] gives it.
///
/// It's this device's own view, and can be behind: a group that has
/// let a device back in looks to that device as it did before, until
/// the checkpoint that says so arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Standing {
    /// It holds a role, and has what the role needs.
    Member,
    /// It holds a role that reads a sealed group, and not the secret
    /// in force. Any peer that has the secret hands it on: there's
    /// nothing to do but stay connected.
    Waiting,
    /// It holds no role. It carries the group for others, and reads
    /// it if the group is open. A device that has just [joined] is
    /// here until the group gives it a role.
    ///
    /// [joined]: Database::join
    Outside,
    /// It holds no role in a sealed group it once read. It keeps what
    /// it read then, and chooses: [`forget`] the group, or
    /// [`relinquish`] what it read and stay to carry it.
    ///
    /// [`forget`]: Database::forget
    /// [`relinquish`]: Database::relinquish
    ShutOut,
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

    /// Where this device stands in `group`: whether it holds a role,
    /// and whether it has what the role needs. `None` if it doesn't
    /// hold the group.
    ///
    /// [`Changed::Members`] says when to ask again.
    ///
    /// [`Changed::Members`]: super::Changed::Members
    #[must_use]
    pub fn standing(&self, group: GroupId) -> Option<Standing> {
        let group = self.groups.get(&group)?;
        let me = self.identity.peer();
        let reads = group.electorate().key_of(me).is_some();

        Some(if !group.has(me) {
            if group.is_shut_out() {
                Standing::ShutOut
            } else {
                Standing::Outside
            }
        } else if group.is_sealed() && reads && !group.reads() {
            Standing::Waiting
        } else {
            Standing::Member
        })
    }

    /// Every peer worth a connection: whoever holds a role in a group
    /// this device holds, and whoever it has asked to be given one.
    /// This device isn't among them.
    ///
    /// It's a place to start, and not a limit. Any peer that holds a
    /// group can be [connected], a role or none.
    ///
    /// [connected]: Self::connected
    #[must_use]
    pub fn members(&self) -> Vec<VerifyingKey> {
        let me = self.identity.peer();
        let named = |group: &Group| {
            let asked = group.asked().into_iter().flat_map(Electorate::members);
            let named: Vec<Peer> = group.electorate().members().chain(asked).collect();

            named
        };
        let peers: BTreeSet<Peer> = self.groups.values().flat_map(named).collect();

        peers
            .into_iter()
            .filter(|&peer| peer != me)
            .map(|peer| peer.id())
            .collect()
    }
}
