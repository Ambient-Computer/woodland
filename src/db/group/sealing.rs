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
    /// force. A checkpoint that names its own height brings in a new
    /// one.
    pub(crate) epoch: u64,
    /// Stands for that secret, so a peer handed one can tell it's the
    /// right one.
    pub(crate) commitment: Commitment,
    /// The secret, for each peer this checkpoint gives it to: everyone
    /// who reads, if the secret is new, and otherwise whoever is newly
    /// given a key.
    pub(crate) grants: BTreeMap<Peer, Grant>,
    /// With a new secret, the one before it, encrypted under the new
    /// one. Whoever is given a secret reads everything before it.
    pub(crate) previous: Option<Vec<u8>>,
}

/// A secret passed from one peer to another, outside any checkpoint.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Handed {
    pub(crate) epoch: u64,
    pub(crate) grant: Grant,
}

/// What a grant is made about: a secret of one group, from one
/// checkpoint.
pub(crate) fn about(group: GroupId, epoch: u64) -> Vec<u8> {
    encoding::encode(&(group, epoch))
}

/// What the secret before the one from `epoch` is wrapped about.
fn about_previous(group: GroupId, epoch: u64) -> Vec<u8> {
    encoding::encode(&("woodland/previous", group, epoch))
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
            previous: None,
        }),
    };

    (secret, encoding::encode(&covers))
}

/// What a group's founding says a device can read of it, before it's
/// handed anything.
pub(super) fn keyring_of(founding: &Checkpoint) -> Keyring {
    let sealed = covers_of(founding).is_some_and(|it| it.sealing.is_some());

    if sealed {
        Keyring::Sealed(BTreeMap::from([(0, None)]))
    } else {
        Keyring::Open
    }
}

fn sealing_of(checkpoint: &Checkpoint) -> Option<Sealing> {
    covers_of(checkpoint).and_then(|it| it.sealing)
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

/// Whether going from `before` to `next` takes the secret away from
/// someone: a peer loses its key, or has it replaced. The secret they
/// held is replaced along with it.
fn takes_away(before: &Electorate, next: &Electorate) -> bool {
    before
        .keys()
        .any(|(peer, key)| next.key_of(peer) != Some(key))
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
        sealing_of(self.chain.tip())
    }

    /// Takes on a secret this device made or was handed.
    pub(crate) fn hold<V: View>(&mut self, epoch: u64, secret: Secret, step: &mut Step<V>) {
        self.keys.hold(epoch, secret);
        self.rekey(step);
    }

    /// Gives every clock the keys as they are now.
    fn rekey<V: View>(&mut self, step: &mut Step<V>) {
        for clock in self.clocks.values_mut() {
            clock.keys = self.keys.clone();
        }
        self.persist(step);
    }

    /// What the next checkpoint of a sealed group says about the
    /// secret, if it gives `electorate` the say. `None` if this device
    /// can't say, for lack of the secret.
    ///
    /// If nobody loses the secret, it's the same one, with a copy for
    /// whoever is newly due one. If somebody does, it's a new one, with
    /// a copy for everyone who still reads.
    pub(super) fn seal_next(
        &self,
        identity: &Identity,
        electorate: &Electorate,
    ) -> Option<Sealing> {
        let now = self.sealing()?;
        let secret = self.keys.secret(now.epoch)?;
        let tip = self.chain.tip();
        let grants = |secret: &Secret, epoch: u64, due: &mut dyn Iterator<Item = Peer>| {
            let about = about(self.id, epoch);
            let grant = |peer: Peer| {
                let key = electorate.key_of(peer)?;

                Some((peer, secret.grant(identity, key, &about)))
            };

            due.filter_map(grant).collect()
        };

        if !takes_away(&tip.electorate, electorate) {
            return Some(Sealing {
                epoch: now.epoch,
                commitment: now.commitment,
                grants: grants(
                    secret,
                    now.epoch,
                    &mut newly_keyed(&tip.electorate, electorate),
                ),
                previous: None,
            });
        }

        // A secret for these readers and no others: an attempt that
        // would have kept someone else never hands them this one.
        let epoch = tip.height + 1;
        let purpose = encoding::encode(&("epoch", self.id, tip.digest(), electorate));
        let next = Secret::make(identity, &purpose);
        let readers = electorate.keys().map(|(peer, _)| peer);

        Some(Sealing {
            epoch,
            commitment: next.commitment(),
            grants: grants(&next, epoch, &mut readers.into_iter()),
            previous: Some(next.wrap(&about_previous(self.id, epoch), secret)),
        })
    }

    /// Whether `checkpoint` says what it should about the secret.
    ///
    /// It keeps the secret in force unless that takes it away from
    /// someone, in which case it brings in a new one. Everyone due a
    /// copy has one, and nobody else. If there's one for this device,
    /// it's the secret the checkpoint commits to, and a new secret
    /// opens the way to the one before it.
    ///
    /// Nobody can check a copy made for someone else. A peer whose copy
    /// is no good was still granted the secret by being listed.
    pub(crate) fn sealed_soundly(&self, identity: &Identity, checkpoint: &Checkpoint) -> bool {
        let next = sealing_of(checkpoint);
        let (Some(now), Some(next)) = (self.sealing(), next.as_ref()) else {
            return !self.is_sealed() && next.is_none();
        };

        let before = &self.chain.tip().electorate;
        let after = &checkpoint.electorate;
        let granted = |peer: Peer| next.grants.contains_key(&peer);
        let listed = |peer: &Peer| after.key_of(*peer).is_some();
        let mine = next.grants.get(&identity.peer());
        let mine = mine.map(|grant| grant.open(identity, &about(self.id, next.epoch)));
        let right = |secret: &Option<Secret>| {
            secret
                .as_ref()
                .is_some_and(|it| it.commitment() == next.commitment)
        };
        let kept = next.epoch == now.epoch
            && next.commitment == now.commitment
            && next.previous.is_none()
            && !takes_away(before, after)
            && newly_keyed(before, after).all(granted);
        let leads_back = || {
            let about = about_previous(self.id, next.epoch);
            let older = |secret: &Secret| secret.unwrap(&about, next.previous.as_deref()?);
            let older = mine.as_ref().map(|secret| secret.as_ref().and_then(older));

            older.is_none_or(|older| older.is_some_and(|it| it.commitment() == now.commitment))
        };
        let replaced = next.epoch == checkpoint.height
            && next.previous.is_some()
            && after.keys().all(|(peer, _)| granted(peer))
            && leads_back();

        (kept || replaced)
            && after.is_sealable()
            && next.grants.keys().all(listed)
            && mine.as_ref().is_none_or(right)
    }

    /// Takes whatever secrets the chain carries for this device that
    /// it doesn't hold yet: its own copies, and from each secret, the
    /// one before. Returns whether it took any.
    pub(super) fn take_secrets(&mut self, identity: &Identity) -> bool {
        if !self.is_sealed() {
            return false;
        }

        let me = identity.peer();
        let said = self.chain.checkpoints().filter_map(|checkpoint| {
            let sealing = sealing_of(checkpoint)?;

            Some((checkpoint.height, sealing))
        });
        let said: BTreeMap<u64, Sealing> = said.collect();
        let mut took = false;
        for sealing in said.values() {
            self.keys.know(sealing.epoch);
            let Some(grant) = sealing.grants.get(&me) else {
                continue;
            };
            if self.keys.secret(sealing.epoch).is_some() {
                continue;
            }

            let secret = grant.open(identity, &about(self.id, sealing.epoch));
            if let Some(secret) = secret.filter(|it| it.commitment() == sealing.commitment) {
                took |= self.keys.hold(sealing.epoch, secret);
            } else {
                tracing::warn!(
                    group = ?self.id,
                    epoch = sealing.epoch,
                    "handed a secret that isn't the one the checkpoint names"
                );
            }
        }

        // From the latest back: each new secret was brought in by a
        // checkpoint that carries the one before.
        for (height, sealing) in said.iter().rev() {
            let before = height.checked_sub(1).and_then(|it| said.get(&it));
            let (Some(wrapped), Some(before)) = (&sealing.previous, before) else {
                continue;
            };
            if sealing.epoch != *height || self.keys.secret(before.epoch).is_some() {
                continue;
            }
            let Some(secret) = self.keys.secret(sealing.epoch) else {
                continue;
            };

            let older = secret.unwrap(&about_previous(self.id, sealing.epoch), wrapped);
            if let Some(older) = older.filter(|it| it.commitment() == before.commitment) {
                took |= self.keys.hold(before.epoch, older);
            }
        }

        took
    }

    /// Takes the secrets the chain carries for this device. If that
    /// lets it read what it couldn't, it starts over from nothing and
    /// is sent the group again: what it kept, it kept without knowing
    /// what any of it did.
    pub(super) async fn unlock<V: View>(
        &mut self,
        identity: &Identity,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let before = self.keys.clone();
        let took = self.take_secrets(identity);
        if self.keys != before {
            self.rekey(step);
        }
        if !took {
            return Ok(());
        }

        tracing::info!(group = ?self.id, "given a secret of the group's");
        if self.is_blind() {
            self.relearn(step).await?;
        }

        Ok(())
    }

    /// The secret this device wants handed to it: the one in force, if
    /// it isn't held.
    pub(super) fn wanting(&self) -> Option<u64> {
        let now = self.sealing()?;

        self.keys.secret(now.epoch).is_none().then_some(now.epoch)
    }

    /// A copy of the secret `peer` asked for, if the group lets it
    /// read and this device has the secret to give. Being listed is
    /// the grant. This is only delivery.
    pub(super) fn hand(&self, identity: &Identity, peer: Peer, epoch: u64) -> Option<Handed> {
        let key = self.chain.tip().electorate.key_of(peer)?;
        let secret = self.keys.secret(epoch)?;
        let grant = secret.grant(identity, key, &about(self.id, epoch));

        Some(Handed { epoch, grant })
    }

    /// Takes a secret a peer handed over, if it's one a checkpoint
    /// held here commits to.
    pub(super) async fn take_handed<V: View>(
        &mut self,
        identity: &Identity,
        handed: Handed,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let names = |checkpoint: &Checkpoint| {
            sealing_of(checkpoint).filter(|sealing| sealing.epoch == handed.epoch)
        };
        let Some(sealing) = self.chain.checkpoints().find_map(names) else {
            return Ok(());
        };
        if self.keys.secret(handed.epoch).is_some() {
            return Ok(());
        }

        let secret = handed.grant.open(identity, &about(self.id, handed.epoch));
        let Some(secret) = secret.filter(|it| it.commitment() == sealing.commitment) else {
            tracing::warn!(group = ?self.id, epoch = handed.epoch, "handed a secret that's no good");

            return Ok(());
        };

        self.hold(handed.epoch, secret, step);
        if self.is_blind() {
            self.relearn(step).await?;
        }

        Ok(())
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
