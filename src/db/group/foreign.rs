//! Clocks that hold a kind of model this device doesn't have.
//!
//! A newer version of the library can write what an older one can't
//! make sense of. The older device keeps those entries and passes them
//! on, and stops vouching for the clock they're in: it can't say what
//! state they lead to. Once it has the kind, it has to find out what it
//! missed.

use crate::model;
use crate::store::{StoreError, View};

use super::super::step::Step;
use super::Group;
use super::clock::Clock;

impl Group {
    /// Catches up with the library, if it has kinds of model it didn't
    /// when this group was last saved. Entries that waited unread read
    /// now. If any were committed unread, what this device has for
    /// that state is wrong, and it fetches it all again.
    pub(crate) async fn outgrow<V: View>(&mut self, step: &mut Step<V>) -> Result<(), StoreError> {
        let latest = model::vocabulary();
        if self.spoke >= latest {
            return Ok(());
        }

        self.spoke = latest;
        if self.clocks.values().any(Clock::is_lost) {
            tracing::info!(group = ?self.id, "fetching again what was committed unread");
            self.relearn(step).await?;
        }
        self.persist(step);

        Ok(())
    }
}
