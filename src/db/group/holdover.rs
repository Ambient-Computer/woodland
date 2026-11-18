//! Keeping a group's blobs while the group is sent again.
//!
//! A device that starts a group over deletes everything it held of
//! it but the chain: what it kept, it kept without knowing what any
//! of it did. That's also everything that referred to the group's
//! blobs, and bytes nothing refers to are deleted. They'd be gone
//! before the group came back, and this device may be the only one
//! that has them.
//!
//! So the blobs the group referred to are listed, and the list
//! refers to them until the device holds again all that some peer
//! does. From then on whatever came back refers to them, and what
//! nothing came back for is let go.

use std::collections::BTreeMap;

use crate::blob::Listed;
use crate::blob::keys as blobs;
use crate::db::entry::{Frontier, reach};
use crate::db::{ClockId, keys};
use crate::store::{StoreError, View};

use super::super::step::Step;
use super::Group;

impl Group {
    /// Lists the blobs the group refers to, before what refers to
    /// them is deleted.
    pub(super) async fn hold_over<V: View>(
        &mut self,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let (start, end) = keys::range(blobs::refs_of(self.id));
        let referred = step.scan(&start, &end).await?;
        let listed = referred.iter().filter_map(|(key, _)| blobs::listed_of(key));
        let listed: Vec<Listed> = listed.collect();
        if listed.is_empty() {
            return Ok(());
        }

        tracing::info!(group = ?self.id, blobs = listed.len(), "keeping blobs while the group is sent again");
        step.save(keys::held_over(self.id), &listed);
        self.held_over = Some(self.chain.tip().height);

        Ok(())
    }

    /// Stops keeping blobs for what the group used to refer to, once
    /// it has been sent again.
    pub(super) fn hand_back<V: View>(&mut self, step: &mut Step<V>) {
        if self.held_over.is_some_and(|from| self.is_level(from)) {
            step.delete(keys::held_over(self.id));
            self.held_over = None;
        }
    }

    /// Whether this device holds everything some peer does, and that
    /// peer has got as far as this device had at `from`.
    fn is_level(&self, from: u64) -> bool {
        let settled = self.staging.is_none() && !self.is_partway();
        let here = self.chain.tip().height;
        let level = |(height, said): (u64, &BTreeMap<ClockId, Frontier>)| {
            (from..=here).contains(&height)
                && said.iter().all(|(id, heads)| self.reaches(*id, heads))
        };
        let wanted = self.interest.as_ref();
        let mut peers = self.remotes.values().filter_map(|peer| peer.holds(wanted));

        settled && peers.any(level)
    }

    /// Whether this device holds `clock` as far as `heads`, as entries
    /// or as the state that stands in for them.
    fn reaches(&self, clock: ClockId, heads: &Frontier) -> bool {
        if !self.wants(clock) {
            return true;
        }

        let held = self.clocks.get(&clock).map(|clock| &clock.history.heads);
        let reached = |held: &Frontier| {
            heads
                .iter()
                .all(|(author, link)| reach(held, *author) >= link.seq)
        };

        heads.is_empty() || held.is_some_and(reached)
    }
}
