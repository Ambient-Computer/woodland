//! Keeping a checkpoint from showing until all of it is in place.
//!
//! A checkpoint can cover more than a step commits, and a snapshot is
//! more than a step moves. Between those steps what's stored is part
//! one state and part the next, and a read of it would be a read of
//! neither.
//!
//! So while that's so, the group is veiled. Each step keeps what its
//! writes replace (see [`Step::keep`]), and reads look there first:
//! they see the state from before, whole. When the last of it is in
//! place the veil lifts, which is one small write, and reads see the
//! new state, whole. What was kept is then thrown away a part at a
//! time, and each key that changed is reported as it goes.
//!
//! A device with nothing committed keeps nothing. Its reads of what's
//! committed find nothing until the veil lifts.

use serde::{Deserialize, Serialize};

use crate::db::keys::{self, Record};
use crate::db::{Changed, Reached};
use crate::store::{StoreError, View};

use super::super::step::Step;
use super::Group;

/// How many kept values are thrown away in a step.
const PAGE: usize = if cfg!(test) { 6 } else { 256 };

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Veil {
    /// Committed state is partly the checkpoint at `at` and partly
    /// what's coming. Reads see the first.
    Up { at: u64, blank: bool },
    /// What was coming is in place and reads see it. What was kept
    /// is on its way out.
    Lifting,
}

/// Where a reader looks first, while a group is veiled.
#[derive(Clone, Debug)]
pub(crate) struct Shown {
    /// What every key of the group starts with.
    pub(crate) prefix: Vec<u8>,
    /// What every kept value's key starts with.
    pub(crate) before: Vec<u8>,
    /// Whether nothing was committed, and so nothing was kept.
    pub(crate) blank: bool,
}

impl Group {
    pub(super) async fn recall_veil<V: View>(&mut self, step: &Step<V>) -> Result<(), StoreError> {
        self.veil = step.load(&keys::veil(self.id)).await?;
        self.settled = self.folded;

        Ok(())
    }

    /// Whether what was kept for the last checkpoint is still on its
    /// way out. The next one waits.
    pub(super) fn is_lifting(&self) -> bool {
        self.veil == Some(Veil::Lifting)
    }

    pub(super) fn is_veiled(&self) -> bool {
        self.veil.is_some()
    }

    /// Where a reader looks first, if the group is veiled.
    pub(crate) fn shown(&self) -> Option<Shown> {
        let Some(Veil::Up { blank, .. }) = self.veil else {
            return None;
        };

        Some(Shown {
            prefix: keys::group(self.id),
            before: keys::before(self.id),
            blank,
        })
    }

    /// Which checkpoint a committed read is of right now.
    pub(crate) fn committed_at(&self) -> Reached {
        let checkpoint = match self.veil {
            Some(Veil::Up { at, .. }) => at,
            _ => self.folded,
        };

        Reached { checkpoint }
    }

    /// Whether a change this step reported is one no reader can see
    /// yet. It's reported again when they can.
    pub(crate) fn hides(&self, change: &Changed) -> bool {
        let unseen = match (self.veil, change) {
            (Some(_), Changed::Checkpoint { group, .. }) => Some(group),
            (
                Some(Veil::Up { .. }),
                Changed::Key {
                    group, committed, ..
                },
            ) => committed.then_some(group),
            _ => None,
        };

        unseen == Some(&self.id)
    }

    /// Comes before a step's indexes are brought up to date. Raises
    /// the veil if the step left committed state in two pieces, and
    /// keeps what the step replaced. Lifts it if the pieces are one
    /// again.
    pub(crate) async fn raise<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        let was = self.veil;
        match (self.veil, self.is_partway()) {
            (None, true) => {
                let (start, end) = keys::range(keys::committed(self.id));
                let blank = step.was_empty(&start, &end).await?;
                let at = self.settled;
                self.veil = Some(Veil::Up { at, blank });
            }
            (Some(Veil::Up { .. }), false) => self.veil = Some(Veil::Lifting),
            _ => {}
        }

        self.keep(step).await?;
        self.note(was, step);

        Ok(())
    }

    /// Ends a step. Keeps what bringing the indexes up to date
    /// replaced, while the veil is up, and throws away some of what
    /// was kept once it's lifted.
    pub(crate) async fn veil<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        let was = self.veil;
        match self.veil {
            Some(Veil::Up { .. }) => self.keep(step).await?,
            Some(Veil::Lifting) => self.lift(step).await?,
            None => self.settled = self.folded,
        }

        self.note(was, step);

        Ok(())
    }

    async fn keep<V: View>(&self, step: &mut Step<V>) -> Result<(), StoreError> {
        if let Some(Veil::Up { blank, .. }) = self.veil {
            let (prefix, before) = (keys::group(self.id), keys::before(self.id));
            step.keep(&prefix, &before, blank).await?;
        }

        Ok(())
    }

    /// Stores where the veil stands, if that's not where it `was`.
    fn note<V: View>(&self, was: Option<Veil>, step: &mut Step<V>) {
        match self.veil {
            veil if veil == was => {}
            Some(veil) => step.save(keys::veil(self.id), &veil),
            None => step.delete(keys::veil(self.id)),
        }
    }

    /// Throws away some of what was kept, and says of each key that
    /// it's changed: readers see what's there now.
    async fn lift<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        let before = keys::before(self.id);
        let (start, end) = keys::range(before.clone());
        let page = step.scan_page(&start, &end, PAGE).await?;
        let last = page.len() < PAGE;
        for (kept, _) in page {
            let key = [&keys::group(self.id)[..], &kept[before.len()..]].concat();
            step.delete(kept);
            let (model, key, levels) = match keys::parse(&key) {
                // What's committed shows through to an optimistic read.
                Some(Record::Committed { model, key, .. }) => (model, key, vec![true, false]),
                Some(Record::Version { model, key, .. }) => (model, key, vec![false]),
                Some(Record::Index {
                    model,
                    key,
                    committed,
                    ..
                }) => (model, key, vec![committed]),
                None => continue,
            };
            for committed in levels {
                step.changed(Changed::Key {
                    group: self.id,
                    model,
                    key: key.clone(),
                    committed,
                });
            }
        }

        if last {
            self.veil = None;
            self.settled = self.folded;
            step.changed(Changed::Checkpoint {
                group: self.id,
                height: self.folded,
            });
        }

        Ok(())
    }
}
