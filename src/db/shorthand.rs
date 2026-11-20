//! What every application would otherwise write for itself: carrying
//! on until a step's work is done, and deleting a row by its key.
//!
//! Nothing here does what the application couldn't. Each is the calls
//! it would make, in the order it would make them.

use crate::model::{ModelId, Query, Table};
use crate::store::Store;

use super::{Database, Error, GroupId, Outputs, Transaction};

impl Outputs {
    /// Takes in what a later step asked for, so that several steps
    /// can be handled as one: every message in the order it was to be
    /// sent, each change once, and whatever was told of transactions
    /// and requests. What's [unfinished], and who there's something
    /// to say to about [blobs], are as the later step left them.
    ///
    /// [unfinished]: Self::unfinished
    /// [blobs]: Self::blobs
    pub fn absorb(&mut self, later: Self) {
        self.sends.extend(later.sends);
        for change in later.changed {
            if !self.changed.contains(&change) {
                self.changed.push(change);
            }
        }
        self.wrote = self.wrote.or(later.wrote);
        self.outcomes.extend(later.outcomes);
        self.reconfigured.extend(later.reconfigured);
        self.unfinished = later.unfinished;
        self.blobs = later.blobs;
    }
}

impl<S: Store> Database<S> {
    /// [Resumes] until nothing is [unfinished], and [adds] what each
    /// step asked for to `outputs`: the loop that follows every call.
    /// Pass what the call returned, then handle `outputs` once.
    ///
    /// Nothing in `outputs` is sent until this returns, and a step
    /// that's far behind has a great deal to do. An application that
    /// would rather send as it goes, or let something else run
    /// between steps, calls [`resume`] itself.
    ///
    /// [Resumes]: Self::resume
    /// [`resume`]: Self::resume
    /// [unfinished]: Outputs::unfinished
    /// [adds]: Outputs::absorb
    ///
    /// # Errors
    ///
    /// If the store fails. What the steps before it asked for is in
    /// `outputs` still.
    pub async fn settle(&mut self, outputs: &mut Outputs) -> Result<(), Error> {
        while outputs.unfinished {
            let resumed = self.resume().await?;
            outputs.absorb(resumed);
        }

        Ok(())
    }

    /// Deletes the row of `table` under `key`, as it is on this device
    /// now. `None` if there's no such row, or no such table, and then
    /// nothing was written.
    ///
    /// It's a read and a [`delete_row`] of what was read, so it keeps
    /// what that promises: if the row was deleted and another put
    /// under the same key before this takes its place in the
    /// timeline, the other row is left alone.
    ///
    /// [`delete_row`]: Transaction::delete_row
    ///
    /// # Errors
    ///
    /// As for [`read`] and [`write`].
    ///
    /// [`read`]: Self::read
    /// [`write`]: Self::write
    pub async fn delete_row(
        &mut self,
        group: GroupId,
        table: ModelId,
        key: &[u8],
    ) -> Result<Option<Outputs>, Error> {
        let answer = self.read(&Query::row(group, table, key)).await?;
        let read = Table::read(answer.found());
        let Some(row) = read.as_ref().and_then(|it| it.row(key)) else {
            return Ok(None);
        };
        let delete = Transaction::new().delete_row(table, row);

        Ok(Some(self.write(group, delete).await?))
    }
}
