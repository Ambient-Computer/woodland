//! Which uncommitted entries happen, kept up as they come and go.
//!
//! An entry happens if what it requires holds, given the entries before
//! it in the timeline that happen. So a verdict can only change when
//! something earlier does: an entry arrives, goes, or changes its own
//! verdict, and writes a key the later one requires something of.
//!
//! That's what's worked out again, and nothing else. An entry arriving
//! in the middle of a long history costs what it changes, not what
//! follows it.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;

use crate::clock::Stamp;
use crate::model::Effect;
use crate::store::{StoreError, View};

use super::super::entry::Outline;
use super::super::step::Step;
use super::clock::Clock;
use super::judge::{self, Held, Place, Written};

#[derive(Clone, Debug, Default)]
pub(super) struct Verdicts {
    /// The uncommitted entries that happen, as storage shows them.
    pub(super) live: BTreeSet<Stamp>,
    /// Whether every verdict needs working out again.
    pub(super) stale: bool,
    /// The uncommitted entries that write each key, or require
    /// something of it.
    touches: BTreeMap<Place, BTreeSet<Stamp>>,
    /// Entries whose verdicts need working out again.
    doubted: BTreeSet<Stamp>,
    /// Keys that hold something else now, for a reason other than an
    /// entry's verdict changing.
    moved: BTreeSet<Place>,
    /// How many verdicts have been worked out one at a time.
    #[cfg(test)]
    pub(super) weighed: usize,
}

/// The key a declaration is about, and what it leaves there if it
/// writes it.
fn place(declared: &crate::model::Declaration) -> Option<(Place, Option<Held>)> {
    match &declared.effect {
        Effect::Require { key, .. } => Some(((declared.model, key.clone()), None)),
        Effect::Write { key, leaves } => Some(((declared.model, key.clone()), Some(*leaves))),
        Effect::Drop => None,
    }
}

impl Verdicts {
    fn index(&mut self, entry: &Outline) {
        for (place, _) in entry.declared.iter().filter_map(place) {
            self.touches.entry(place).or_default().insert(entry.stamp());
        }
    }

    /// An entry has come. Whether it happens is yet to be worked out.
    pub(super) fn arrived(&mut self, entry: &Outline) {
        self.index(entry);
        self.doubted.insert(entry.stamp());
    }

    /// An entry is uncommitted no longer: it was committed, or never
    /// will be. Either way what it wrote isn't what later entries find.
    pub(super) fn gone(&mut self, entry: &Outline) {
        let stamp = entry.stamp();
        self.live.remove(&stamp);
        self.doubted.remove(&stamp);
        for (place, leaves) in entry.declared.iter().filter_map(place) {
            if let Some(stamps) = self.touches.get_mut(&place) {
                stamps.remove(&stamp);
                if stamps.is_empty() {
                    self.touches.remove(&place);
                }
            }
            if leaves.is_some() {
                self.moved.insert(place);
            }
        }
    }

    /// These keys hold something else now.
    pub(super) fn moved(&mut self, places: impl IntoIterator<Item = Place>) {
        self.moved.extend(places);
    }

    /// Whether there's a verdict to work out.
    fn is_settled(&self) -> bool {
        !self.stale && self.doubted.is_empty() && self.moved.is_empty()
    }
}

impl Clock {
    /// Which uncommitted entries happen, worked out from the start.
    pub(super) async fn judged<V: View>(
        &self,
        step: &Step<V>,
    ) -> Result<BTreeSet<Stamp>, StoreError> {
        let mut written = Written::on(&self.dropped);
        let mut live = BTreeSet::new();
        if self.is_blind() {
            return Ok(live);
        }

        for (&stamp, entry) in &self.history.uncommitted {
            if judge::weigh(self.group, entry, &written, step).await? {
                written.apply(&entry.declared);
                live.insert(stamp);
            }
        }

        Ok(live)
    }

    /// Takes what storage shows as what happens: it was stored along
    /// with the entries it shows.
    pub(super) async fn recall<V: View>(&mut self, step: &Step<V>) -> Result<(), StoreError> {
        self.verdicts = Verdicts {
            live: self.judged(step).await?,
            ..Verdicts::default()
        };
        for entry in self.history.uncommitted.values() {
            self.verdicts.index(entry);
        }

        Ok(())
    }

    /// Works out every verdict that could have changed, and makes
    /// storage show exactly the entries that happen.
    pub(super) async fn rejudge<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        if self.verdicts.is_settled() {
            return Ok(());
        }
        // An entry this device can't read hides everything, and
        // committing one that takes steps changes keys it never named.
        if self.verdicts.stale || self.is_blind() {
            return self.rejudge_all(step).await;
        }

        let mut doubted = std::mem::take(&mut self.verdicts.doubted);
        for place in std::mem::take(&mut self.verdicts.moved) {
            doubted.extend(self.verdicts.touches.get(&place).into_iter().flatten());
        }

        // In timeline order: a verdict that changes can only change
        // later ones.
        while let Some(stamp) = doubted.pop_first() {
            let Some(entry) = self.history.uncommitted.get(&stamp) else {
                continue;
            };

            let mut written = Written::on(&self.dropped);
            for (place, _) in entry.declared.iter().filter_map(place) {
                if let Some(held) = self.left_before(&place, stamp) {
                    written.hold(place, held);
                }
            }
            let happens = judge::weigh(self.group, entry, &written, step).await?;
            #[cfg(test)]
            {
                self.verdicts.weighed += 1;
            }
            if happens == self.verdicts.live.contains(&stamp) {
                continue;
            }

            if happens {
                let operations = self.operations(entry, step).await?;
                judge::show(self.group, stamp, &operations, step);
                self.verdicts.live.insert(stamp);
            } else {
                judge::hide(self.group, entry, step);
                self.verdicts.live.remove(&stamp);
            }
            judge::notify(self.group, entry, false, step);

            let after = (Bound::Excluded(stamp), Bound::Unbounded);
            for (place, leaves) in entry.declared.iter().filter_map(place) {
                let later = self
                    .verdicts
                    .touches
                    .get(&place)
                    .filter(|_| leaves.is_some());
                doubted.extend(later.into_iter().flat_map(|stamps| stamps.range(after)));
            }
        }

        #[cfg(test)]
        assert_eq!(
            self.verdicts.live,
            self.judged(step).await?,
            "verdicts kept up as entries came and went aren't the ones worked out from the start"
        );

        Ok(())
    }

    /// What the last entry before `stamp` that happens, and writes
    /// `place`, leaves there. `None` if there isn't one, and it holds
    /// what's committed.
    fn left_before(&self, place: &Place, stamp: Stamp) -> Option<Held> {
        let earlier = self.verdicts.touches.get(place)?.range(..stamp).rev();
        let earlier = earlier.filter(|stamp| self.verdicts.live.contains(stamp));
        let mut left = earlier.filter_map(|stamp| {
            let declared = &self.history.uncommitted.get(stamp)?.declared;
            let writes = declared.iter().filter_map(self::place);
            let writes = writes.filter(|(written, _)| written == place);

            writes.filter_map(|(_, leaves)| leaves).next_back()
        });

        left.next()
    }

    async fn rejudge_all<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        let live = self.judged(step).await?;
        for stamp in live.symmetric_difference(&self.verdicts.live) {
            let Some(entry) = self.history.uncommitted.get(stamp) else {
                continue;
            };

            if live.contains(stamp) {
                let operations = self.operations(entry, step).await?;
                judge::show(self.group, *stamp, &operations, step);
            } else {
                judge::hide(self.group, entry, step);
            }
            judge::notify(self.group, entry, false, step);
        }
        self.verdicts = Verdicts {
            live,
            #[cfg(test)]
            weighed: self.verdicts.weighed,
            ..Verdicts::default()
        };
        for entry in self.history.uncommitted.values() {
            self.verdicts.index(entry);
        }

        Ok(())
    }
}
