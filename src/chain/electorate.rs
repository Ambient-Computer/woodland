use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::actor::Peer;
use crate::db::ClockId;
use crate::seal::SealingKey;

/// What a peer may do in a group, and how much it counts toward a
/// quorum.
///
/// A contributor writes, and checks what it votes for. A witness never
/// writes, and votes for the order of checkpoints without vouching for
/// what they cover: weight for a device that's always on and holds
/// nothing. A reader does neither, and is named so that it can read a
/// sealed group.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Role {
    weight: u32,
    part: Part,
    /// The key it's sent a sealed group's secrets under.
    key: Option<SealingKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum Part {
    /// Writes to these clocks. `None` is every clock.
    Contributor(Option<BTreeSet<ClockId>>),
    Witness,
    Reader,
}

impl Role {
    /// Writes to every clock. With no weight it has no say in what's
    /// committed, and writes all the same.
    #[must_use]
    pub fn contributor(weight: u32) -> Self {
        Self {
            weight,
            part: Part::Contributor(None),
            key: None,
        }
    }

    /// Counts toward a quorum, and never writes.
    #[must_use]
    pub fn witness(weight: u32) -> Self {
        Self {
            weight,
            part: Part::Witness,
            key: None,
        }
    }

    /// Reads a sealed group, and does nothing else: it never writes
    /// and has no say. `key` is its [sealing key].
    ///
    /// [sealing key]: crate::Database::sealing_key
    #[must_use]
    pub fn reader(key: SealingKey) -> Self {
        Self {
            weight: 0,
            part: Part::Reader,
            key: Some(key),
        }
    }

    /// Lets a contributor read a sealed group, which every contributor
    /// to one has to. `key` is its [sealing key]. A witness has no use
    /// for one: it votes on what it can't read.
    ///
    /// [sealing key]: crate::Database::sealing_key
    #[must_use]
    pub fn sealed_to(mut self, key: SealingKey) -> Self {
        if matches!(self.part, Part::Contributor(_)) {
            self.key = Some(key);
        }
        self
    }

    /// Narrows a contributor to `clocks`. Whatever it wrote elsewhere
    /// that isn't committed by then is dropped, as if it had been
    /// removed there.
    #[must_use]
    pub fn only(mut self, clocks: impl IntoIterator<Item = ClockId>) -> Self {
        if let Part::Contributor(allowed) = &mut self.part {
            *allowed = Some(clocks.into_iter().collect());
        }
        self
    }

    /// How much it counts toward a quorum. With none, it has no say
    /// in what's committed or in who the members are.
    #[must_use]
    pub fn weight(&self) -> u32 {
        self.weight
    }

    /// Whether it writes: to every clock, or to [some].
    ///
    /// [some]: Self::clocks
    #[must_use]
    pub fn is_contributor(&self) -> bool {
        matches!(self.part, Part::Contributor(_))
    }

    /// Whether it votes without writing or reading.
    #[must_use]
    pub fn is_witness(&self) -> bool {
        self.part == Part::Witness
    }

    /// Whether it's named only so that it can read a sealed group.
    #[must_use]
    pub fn is_reader(&self) -> bool {
        self.part == Part::Reader
    }

    /// The clocks a contributor is [narrowed] to. `None` if it writes
    /// to every clock, or to none because it isn't a contributor.
    ///
    /// [narrowed]: Self::only
    #[must_use]
    pub fn clocks(&self) -> Option<&BTreeSet<ClockId>> {
        match &self.part {
            Part::Contributor(clocks) => clocks.as_ref(),
            Part::Witness | Part::Reader => None,
        }
    }

    /// The key it's sent a sealed group's secrets under, if it reads
    /// one.
    #[must_use]
    pub fn sealing_key(&self) -> Option<&SealingKey> {
        self.key.as_ref()
    }

    /// Whether it may write to `clock`.
    #[must_use]
    pub fn writes(&self, clock: ClockId) -> bool {
        match &self.part {
            Part::Contributor(clocks) => clocks.as_ref().is_none_or(|it| it.contains(&clock)),
            Part::Witness | Part::Reader => false,
        }
    }

    /// Whether there's anything this lets a peer do.
    fn is_something(&self) -> bool {
        self.weight > 0 || !matches!(self.part, Part::Witness)
    }
}

/// A group's configuration: who has a say in the next checkpoint, how
/// much each one counts, and who may write where.
///
/// Weights are the application's choice. A device that's always on can
/// outweigh the rest, so that it and any one other are enough. A peer
/// with more than half the weight decides alone, and equal weights with
/// everyone needed is unanimity. None of those is a special case.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Electorate(BTreeMap<Peer, Role>);

/// Who may write to one clock.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Writers<'a> {
    electorate: &'a Electorate,
    clock: ClockId,
    barred: Option<&'a BTreeSet<Peer>>,
}

impl<'a> Writers<'a> {
    pub(crate) fn includes(&self, peer: Peer) -> bool {
        let role = self.electorate.0.get(&peer);
        let barred = self.barred.is_some_and(|barred| barred.contains(&peer));

        !barred && role.is_some_and(|role| role.writes(self.clock))
    }

    /// The same, less anyone in `barred`, whatever their role says.
    pub(crate) fn except(mut self, barred: &'a BTreeSet<Peer>) -> Self {
        self.barred = Some(barred);
        self
    }
}

impl Electorate {
    /// Contributors to every clock, with these weights. A peer with no
    /// weight has no say, so it isn't kept.
    pub(crate) fn new(weights: impl IntoIterator<Item = (Peer, u32)>) -> Self {
        let weights = weights.into_iter().filter(|(_, weight)| *weight > 0);

        Self::of(weights.map(|(peer, weight)| (peer, Role::contributor(weight))))
    }

    /// A role that lets its peer do nothing isn't kept.
    pub(crate) fn of(roles: impl IntoIterator<Item = (Peer, Role)>) -> Self {
        let roles = roles.into_iter();

        Self(roles.filter(|(_, role)| role.is_something()).collect())
    }

    /// Whether `peer` has a say in the next checkpoint.
    pub(crate) fn includes(&self, peer: Peer) -> bool {
        self.0.get(&peer).is_some_and(|role| role.weight > 0)
    }

    /// Whether `peer` votes without checking what it votes for.
    pub(crate) fn is_witness(&self, peer: Peer) -> bool {
        let role = self.0.get(&peer);

        role.is_some_and(|role| role.part == Part::Witness)
    }

    /// Whether `peer` is named only so that it can read.
    pub(crate) fn is_reader(&self, peer: Peer) -> bool {
        let role = self.0.get(&peer);

        role.is_some_and(|role| role.part == Part::Reader)
    }

    /// Everyone a sealed group's secrets go to, and the key each is
    /// sent them under.
    pub(crate) fn keys(&self) -> impl Iterator<Item = (Peer, &SealingKey)> + '_ {
        let keyed = self.0.iter().filter(|(_, role)| role.key.is_some());

        keyed.filter_map(|(&peer, role)| Some((peer, role.key.as_ref()?)))
    }

    /// The key `peer` is sent secrets under, if it's given one.
    pub(crate) fn key_of(&self, peer: Peer) -> Option<&SealingKey> {
        self.0.get(&peer).and_then(|role| role.key.as_ref())
    }

    /// Whether everyone who reads has a key to be sent secrets under:
    /// every contributor and every reader. A sealed group needs that.
    pub(crate) fn is_sealable(&self) -> bool {
        let reads = |role: &&Role| !matches!(role.part, Part::Witness);

        self.0.values().filter(reads).all(|role| role.key.is_some())
    }

    /// Who may write to `clock`.
    pub(crate) fn writers(&self, clock: ClockId) -> Writers<'_> {
        Writers {
            electorate: self,
            clock,
            barred: None,
        }
    }

    /// Everyone with a say.
    pub(crate) fn peers(&self) -> impl Iterator<Item = Peer> + '_ {
        let voters = self.0.iter().filter(|(_, role)| role.weight > 0);

        voters.map(|(&peer, _)| peer)
    }

    /// The same, with none of `peers` holding any role.
    pub(crate) fn without(&self, peers: &BTreeSet<Peer>) -> Self {
        let kept = self.0.iter().filter(|(peer, _)| !peers.contains(peer));

        Self(kept.map(|(&peer, role)| (peer, role.clone())).collect())
    }

    /// Whether `peer` has any role at all.
    pub(crate) fn has(&self, peer: Peer) -> bool {
        self.0.contains_key(&peer)
    }

    /// Everyone with a role, and the role.
    pub(crate) fn roles(&self) -> impl Iterator<Item = (Peer, &Role)> + '_ {
        self.0.iter().map(|(&peer, role)| (peer, role))
    }

    /// Everyone with a role, with a say or not.
    pub(crate) fn members(&self) -> impl Iterator<Item = Peer> + '_ {
        self.0.keys().copied()
    }

    /// Whether a checkpoint could ever follow one that carries this.
    /// It takes someone with a say to vote for it, and a contributor to
    /// work out what it would commit.
    pub(crate) fn is_viable(&self) -> bool {
        let contributes = |role: &Role| matches!(role.part, Part::Contributor(_));
        // A key its owner didn't sign for is someone else's.
        let vouched = self.keys().all(|(peer, key)| key.is_vouched_by(peer));

        self.peers().next().is_some() && self.0.values().any(contributes) && vouched
    }

    /// Whether `holders` carry more than half the total weight.
    ///
    /// Any two such sets share a member. That overlap is what agreement's
    /// safety rests on: the shared member has seen both sides.
    pub(crate) fn has_quorum(&self, holders: &BTreeSet<Peer>) -> bool {
        let weight = |peer: &Peer| u64::from(self.0.get(peer).map_or(0, |role| role.weight));
        let total: u64 = self.0.keys().map(weight).sum();
        let held: u64 = holders.iter().map(weight).sum();

        held * 2 > total
    }
}
