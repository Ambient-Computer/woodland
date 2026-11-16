//! A sealed group's committed state, on its way between devices.
//!
//! A snapshot of a sealed group travels encrypted, a piece at a time.
//! A device that reads the group opens each piece, and takes the
//! whole for its state once it adds up to what the checkpoint names.
//! A device that doesn't keeps the pieces as they came and passes them
//! on. It can't tell whether they're the state the checkpoint names,
//! and doesn't need to: whoever it passes them to can.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::db::keys;
use crate::encoding;
use crate::store::{StoreError, View};

use super::super::message::{Piece, SealedPiece};
use super::super::step::Step;
use super::Group;
use super::commit::covers_of;
use super::compact::Sending;
use super::sync::Remote;

/// What a device makes of a piece of a snapshot.
pub(super) enum Unsealed {
    /// Its values, in the clear.
    Read(Piece),
    /// It's sealed and this device can't read the group. It's kept as
    /// it is.
    Carried(Piece),
    /// It isn't what it should be, or isn't for this device.
    Refused,
}

/// A piece as a device that can't read it keeps it.
#[derive(Serialize, Deserialize)]
struct Kept {
    upto: Option<Vec<u8>>,
    sealed: SealedPiece,
}

/// What a piece's contents are sealed about: the snapshot it's part
/// of, and where in it.
fn about_piece(piece: &Piece) -> Vec<u8> {
    encoding::encode(&("woodland/piece", piece.height, &piece.start))
}

/// What the name of a place in a snapshot is sealed about.
fn about_cursor(height: u64) -> Vec<u8> {
    encoding::encode(&("woodland/cursor", height))
}

/// Where a kept piece is stored: by where it starts.
fn kept_at(group: crate::db::GroupId, start: Option<&Vec<u8>>) -> Vec<u8> {
    let start = start.map_or(vec![0], |start| [&[1][..], start].concat());

    [keys::sealed(group), start].concat()
}

impl Group {
    /// Makes a piece ready to travel: as it is in an open group, and
    /// in a sealed one, with everything it says encrypted. `None` if
    /// this device lacks the secret to do that.
    pub(super) fn seal_piece(&self, piece: Piece) -> Option<Piece> {
        if !self.is_sealed() {
            return Some(piece);
        }

        let secret = self.secret_at(piece.height)?;
        let upto = piece.upto.as_ref();
        let upto = upto.map(|key| secret.seal(&about_cursor(piece.height), key));
        let body = encoding::encode(&(&piece.values, &piece.end));
        let sealed = SealedPiece {
            body: secret.seal(&about_piece(&piece), &body),
            last: piece.end.is_some(),
        };

        Some(Piece {
            height: piece.height,
            start: piece.start,
            values: Vec::new(),
            upto,
            end: None,
            sealed: Some(sealed),
        })
    }

    /// Undoes [`seal_piece`], if this device can.
    ///
    /// [`seal_piece`]: Self::seal_piece
    pub(super) fn open_piece(&self, piece: Piece) -> Unsealed {
        let Some(sealed) = &piece.sealed else {
            // Nothing travels in the clear in a sealed group.
            return if self.is_sealed() {
                Unsealed::Refused
            } else {
                Unsealed::Read(piece)
            };
        };
        if !self.is_sealed() {
            return Unsealed::Refused;
        }
        let Some(secret) = self.secret_at(piece.height) else {
            return if self.carries() {
                Unsealed::Carried(piece)
            } else {
                Unsealed::Refused
            };
        };

        let body = secret.open(&about_piece(&piece), &sealed.body);
        let Some((values, end)) = body.and_then(|body| encoding::decode(&body)) else {
            tracing::warn!(group = ?self.id, height = piece.height, "refused a piece of a snapshot that doesn't open");

            return Unsealed::Refused;
        };
        let end: Option<_> = end;

        if end.is_some() == sealed.last {
            Unsealed::Read(Piece {
                values,
                end,
                sealed: None,
                ..piece
            })
        } else {
            Unsealed::Refused
        }
    }

    /// Keeps a piece this device can't read. Once the last one is in,
    /// the device takes the checkpoint's word for what it covers, and
    /// has the pieces to pass on.
    pub(super) async fn carry<V: View>(
        &mut self,
        piece: Piece,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let Some(sealed) = piece.sealed.clone() else {
            return Ok(());
        };
        let Some(mut staging) = self.next_piece(&piece, step).await? else {
            return Ok(());
        };

        let last = sealed.last;
        let kept = Kept {
            upto: piece.upto.clone(),
            sealed,
        };
        let key = kept_at(self.id, piece.start.as_ref());
        let staged = keys::staged_value(self.id, &key[keys::sealed(self.id).len()..]);
        step.save(staged, &kept);
        staging.upto = piece.upto;
        if !last {
            self.staging = Some(staging);
            return Ok(());
        }

        let Some(covers) = self.chain.at(staging.height).and_then(covers_of) else {
            return self.unstage(step).await;
        };

        let (start, end) = keys::range(keys::sealed(self.id));
        for (key, _) in step.scan(&start, &end).await? {
            step.delete(key);
        }
        let prefix = keys::staged(self.id);
        let (start, end) = keys::range(prefix.clone());
        for (key, value) in step.scan(&start, &end).await? {
            step.put(
                [&keys::sealed(self.id)[..], &key[prefix.len()..]].concat(),
                value,
            );
            step.delete(key);
        }

        for (&id, covered) in &covers.clocks {
            let clock = self.clock(id);
            clock
                .adopt(&covered.frontier, BTreeSet::default(), step)
                .await?;
        }
        self.carrying = Some(staging.height);
        self.caught_up(staging.height, step);

        Ok(())
    }

    /// The next piece of the snapshot this device keeps without being
    /// able to read it, for a peer that's behind it.
    pub(super) async fn carried_piece<V: View>(
        &self,
        remote: &Remote,
        step: &Step<V>,
    ) -> Result<Option<(Piece, Sending)>, StoreError> {
        let Some(height) = self.carrying.filter(|it| *it == self.base) else {
            return Ok(None);
        };
        if !remote.ready_for(height) {
            return Ok(None);
        }

        let start = remote.sent_of(height).and_then(|it| it.upto.clone());
        let kept = step.load::<Kept>(&kept_at(self.id, start.as_ref())).await?;
        let Some(kept) = kept else {
            return Ok(None);
        };
        let sending = Sending {
            height,
            upto: kept.upto.clone(),
            at: None,
            done: kept.sealed.last,
        };
        let piece = Piece {
            height,
            start,
            values: Vec::new(),
            upto: kept.upto,
            end: None,
            sealed: Some(kept.sealed),
        };

        Ok(Some((piece, sending)))
    }
}
