//! Sealed groups: the secret, and how it reaches whoever should hold
//! it.
//!
//! A sealed group's founding checkpoint commits to a secret, and so
//! does every checkpoint after it. A checkpoint that gives a peer a
//! sealing key carries the secret encrypted to that key, so being
//! listed is what grants it and the chain is what delivers it.
//!
//! A device without the secret still holds the group. It checks every
//! signature and link, keeps and passes on what it's sent, and commits
//! checkpoints without knowing what they did.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::actor::{Identity, Peer};
use crate::chain::{Checkpoint, Electorate};
use crate::db::{Changed, ClockId, GroupId, keys};
use crate::encoding;
use crate::seal::{Commitment, Grant, Keyring, Secret};
use crate::store::{StoreError, View};

use super::super::step::Step;
use super::Group;
use super::clock::Clock;
use super::commit::{Covers, covers_of};

/// What a checkpoint of a sealed group says about its secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Sealing {
    /// The height of the checkpoint that brought in the secret in
    /// force.
    pub(crate) epoch: u64,
    /// Stands for that secret, so a peer handed one can tell it's the
    /// right one.
    pub(crate) commitment: Commitment,
    /// The secret, for each peer this checkpoint gives a key.
    pub(crate) grants: BTreeMap<Peer, Grant>,
}

/// What a grant is made about: a secret of one group, from one
/// checkpoint.
pub(crate) fn about(group: GroupId, epoch: u64) -> Vec<u8> {
    encoding::encode(&(group, epoch))
}

/// The secret a sealed group starts with, and the body of the founding
/// checkpoint that commits to it.
pub(crate) fn found(identity: &Identity, seed: [u8; 32]) -> (Secret, Vec<u8>) {
    let secret = Secret::make(identity, &[&b"founding"[..], &seed].concat());
    let covers = Covers {
        clocks: BTreeMap::new(),
        sealing: Some(Sealing {
            epoch: 0,
            commitment: secret.commitment(),
            grants: BTreeMap::new(),
        }),
    };

    (secret, encoding::encode(&covers))
}

/// What a group's founding says a device can read of it, before it's
/// handed anything.
pub(super) fn keyring_of(founding: &Checkpoint) -> Keyring {
    let sealed = covers_of(founding).is_some_and(|it| it.sealing.is_some());

    if sealed {
        Keyring::Sealed(BTreeMap::new())
    } else {
        Keyring::Open
    }
}

/// Everyone `next` gives a sealing key that `before` didn't give that
/// same key.
fn newly_keyed<'a>(
    before: &'a Electorate,
    next: &'a Electorate,
) -> impl Iterator<Item = Peer> + 'a {
    let new = next
        .keys()
        .filter(|(peer, key)| before.key_of(*peer) != Some(*key));

    new.map(|(peer, _)| peer)
}

impl Group {
    pub(crate) fn is_sealed(&self) -> bool {
        self.keys.is_sealed()
    }

    /// Whether this device can read what's written from here on: the
    /// group is open, or it holds the secret in force.
    pub(crate) fn reads(&self) -> bool {
        let Some(now) = self.sealing() else {
            return !self.is_sealed();
        };

        self.keys.secret(now.epoch).is_some()
    }

    /// What the latest checkpoint says about the secret.
    fn sealing(&self) -> Option<Sealing> {
        covers_of(self.chain.tip()).and_then(|it| it.sealing)
    }

    /// Takes on a secret this device made or was handed.
    pub(crate) fn hold<V: View>(&mut self, epoch: u64, secret: Secret, step: &mut Step<V>) {
        self.keys.hold(epoch, secret);
        for clock in self.clocks.values_mut() {
            clock.keys = self.keys.clone();
        }
        self.persist(step);
    }

    /// What the next checkpoint of a sealed group says about the
    /// secret, if it gives `electorate` the say: the same secret, and a
    /// copy for whoever is newly due one. `None` if this device can't
    /// say, for lack of the secret.
    pub(super) fn seal_next(
        &self,
        identity: &Identity,
        electorate: &Electorate,
    ) -> Option<Sealing> {
        let now = self.sealing()?;
        let secret = self.keys.secret(now.epoch)?;

        let about = about(self.id, now.epoch);
        let before = &self.chain.tip().electorate;
        let grant = |peer: Peer| {
            Some((
                peer,
                secret.grant(identity, electorate.key_of(peer)?, &about),
            ))
        };

        Some(Sealing {
            epoch: now.epoch,
            commitment: now.commitment,
            grants: newly_keyed(before, electorate).filter_map(grant).collect(),
        })
    }

    /// Whether `checkpoint` says what it should about the secret: the
    /// one in force, a copy for everyone newly due one, and, if there's
    /// one for this device, the right secret in it.
    ///
    /// Nobody can check a copy made for someone else. A peer whose copy
    /// is no good was still granted the secret by being listed.
    pub(crate) fn sealed_soundly(&self, identity: &Identity, checkpoint: &Checkpoint) -> bool {
        let next = covers_of(checkpoint).and_then(|it| it.sealing);
        let (Some(now), Some(next)) = (self.sealing(), next.as_ref()) else {
            return !self.is_sealed() && next.is_none();
        };

        let before = &self.chain.tip().electorate;
        let granted = |peer: Peer| next.grants.contains_key(&peer);
        let mine = next.grants.get(&identity.peer());
        let right = |grant: &Grant| {
            let secret = grant.open(identity, &about(self.id, next.epoch));

            secret.is_some_and(|it| it.commitment() == next.commitment)
        };

        next.epoch == now.epoch
            && next.commitment == now.commitment
            && checkpoint.electorate.is_sealable()
            && newly_keyed(before, &checkpoint.electorate).all(granted)
            && mine.is_none_or(right)
    }

    /// Takes whatever secrets the chain carries for this device that
    /// it doesn't hold yet. If that lets it read what it couldn't, it
    /// starts over from nothing and is sent the group again: what it
    /// kept, it kept without knowing what any of it did.
    pub(super) async fn unlock<V: View>(
        &mut self,
        identity: &Identity,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        if !self.is_sealed() || self.reads() {
            return Ok(());
        }

        let me = identity.peer();
        let mut found = Vec::new();
        for checkpoint in self.chain.checkpoints() {
            let Some(sealing) = covers_of(checkpoint).and_then(|it| it.sealing) else {
                continue;
            };
            let Some(grant) = sealing.grants.get(&me) else {
                continue;
            };
            if self.keys.secret(sealing.epoch).is_some() {
                continue;
            }

            let secret = grant.open(identity, &about(self.id, sealing.epoch));
            if let Some(secret) = secret.filter(|it| it.commitment() == sealing.commitment) {
                found.push((sealing.epoch, secret));
            } else {
                tracing::warn!(
                    group = ?self.id,
                    height = checkpoint.height,
                    "handed a secret that isn't the one the checkpoint names"
                );
            }
        }
        if found.is_empty() {
            return Ok(());
        }

        for (epoch, secret) in found {
            self.hold(epoch, secret, step);
        }
        tracing::info!(group = ?self.id, "given the group's secret");

        self.relearn(step).await
    }

    /// Forgets everything but the chain, so that it's all sent again.
    async fn relearn<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        let clocks: Vec<ClockId> = self.clocks.keys().copied().collect();
        let mut ranges = vec![
            keys::committed(self.id),
            keys::uncommitted(self.id),
            keys::logs(self.id),
            keys::staged(self.id),
        ];
        ranges.extend(clocks.into_iter().map(|id| keys::state(self.id, id)));
        for prefix in ranges {
            let (start, end) = keys::range(prefix);
            for (key, _) in step.scan(&start, &end).await? {
                step.delete(key);
            }
        }

        self.clocks.clear();
        self.folded = 0;
        self.base = 0;
        self.staging = None;
        self.sound.clear();
        self.said = true;
        step.changed(Changed::Everything { group: self.id });

        Ok(())
    }

    /// The checkpoint this device would propose to give `electorate`
    /// the say, covering nothing new, after `tamper` has had its way
    /// with what it says about the secret.
    #[cfg(test)]
    pub(crate) fn candidate(
        &self,
        identity: &Identity,
        electorate: Electorate,
        tamper: impl FnOnce(&mut Option<Sealing>),
    ) -> Checkpoint {
        let mut covers = covers_of(self.chain.tip()).expect("a checkpoint it made");
        covers.sealing = self.seal_next(identity, &electorate);
        tamper(&mut covers.sealing);

        self.chain
            .tip()
            .extend(electorate, encoding::encode(&covers))
    }

    /// Starts an attempt at [`candidate`], as a proposer that's up to
    /// no good would.
    ///
    /// [`candidate`]: Self::candidate
    #[cfg(test)]
    pub(crate) fn put_forward(
        &mut self,
        identity: &Identity,
        electorate: Electorate,
        tamper: impl FnOnce(&mut Option<Sealing>),
    ) {
        let candidate = self.candidate(identity, electorate, tamper);
        let begun = self
            .chain
            .propose(identity, candidate.electorate, candidate.body);
        assert!(begun, "a member proposing a viable electorate");
        self.said = true;
    }

    /// Whether some clock here has an entry this device can't read.
    pub(super) fn is_blind(&self) -> bool {
        self.clocks.values().any(Clock::is_blind)
    }
}
