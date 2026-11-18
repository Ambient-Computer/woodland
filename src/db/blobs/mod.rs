//! The bytes of blobs: bringing them in, reading them, and letting
//! them go. `docs/spec/blobs.md` is the design.

mod fetch;

use super::{Changed, Database, Error, Outputs, Step};
use crate::BlobRef;
use crate::blob::kept::{self, Bytes, Kept};
use crate::store::Store;

pub(super) use fetch::{Blobs, settle};

/// Names an import on this device. It means nothing anywhere else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ImportId(u64);

/// An import this device is holding: see [`Database::imports`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    pub id: ImportId,
    /// How many bytes it has taken in.
    pub length: u64,
    /// The reference to the blob, if the import is finished. Nothing
    /// refers to it yet.
    pub blob: Option<BlobRef>,
}

fn import((number, kept): (u64, Kept)) -> Import {
    Import {
        id: ImportId(number),
        length: kept.blob.map_or(kept.taken, |blob| blob.length),
        blob: kept.reference(),
    }
}

impl<S: Store> Database<S> {
    /// The most bytes one step takes into an import, or reads of a
    /// blob.
    pub const BLOB_STEP: usize = kept::MOST;

    /// Starts bringing bytes in to be a blob in open groups.
    ///
    /// Give it the bytes in order with [`import`], and when they're
    /// all in, [`finish_import`] says what to refer to them by.
    ///
    /// An import is kept from this step on, across restarts, until
    /// it's [abandoned] or something refers to its blob. Nothing else
    /// lets go of it: [`imports`] says which there are.
    ///
    /// [`import`]: Self::import
    /// [`finish_import`]: Self::finish_import
    /// [abandoned]: Self::abandon_import
    /// [`imports`]: Self::imports
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn begin_import(&mut self) -> Result<(ImportId, Outputs), Error> {
        let mut step = self.begin()?;
        let begun = kept::begin(&mut step, None).await;
        let number = self.guard(begun)?;

        Ok((ImportId(number), self.finish(step).await?))
    }

    /// Takes in the next bytes of an import, and says how many: all
    /// of `bytes`, or the first [`BLOB_STEP`] of them. Call it again
    /// with the rest.
    ///
    /// A device that stops carries on from [`Import::length`].
    ///
    /// [`BLOB_STEP`]: Self::BLOB_STEP
    ///
    /// # Errors
    ///
    /// If there's no such import, or the store fails.
    pub async fn import(&mut self, id: ImportId, bytes: &[u8]) -> Result<(usize, Outputs), Error> {
        let mut step = self.begin()?;
        let taken = kept::append(&mut step, id.0, bytes).await;
        let taken = self.guard(taken)?.ok_or(Error::UnknownImport)?;

        Ok((taken, self.finish(step).await?))
    }

    /// Ends an import. Its bytes are a blob, and this is the
    /// reference to it, for a transaction to write.
    ///
    /// The import goes on holding the blob until something refers to
    /// it, or it's [abandoned].
    ///
    /// [abandoned]: Self::abandon_import
    ///
    /// # Errors
    ///
    /// If there's no such import, or the store fails.
    pub async fn finish_import(&mut self, id: ImportId) -> Result<(BlobRef, Outputs), Error> {
        let mut step = self.begin()?;
        let finished = kept::finish(&mut step, id.0).await;
        let blob = self.guard(finished)?.ok_or(Error::UnknownImport)?;
        step.changed(Changed::Blob {
            name: blob.name(),
            length: blob.length(),
        });

        Ok((blob, self.finish(step).await?))
    }

    /// Lets go of an import, finished or not. Its bytes are deleted,
    /// unless something has come to refer to them.
    ///
    /// # Errors
    ///
    /// If there's no such import, or the store fails.
    pub async fn abandon_import(&mut self, id: ImportId) -> Result<Outputs, Error> {
        let mut step = self.begin()?;
        let abandoned = kept::abandon(&mut step, id.0).await;
        if !self.guard(abandoned)? {
            return Err(Error::UnknownImport);
        }

        self.finish(step).await
    }

    /// The imports this device is holding: those that aren't
    /// finished, and those that are and that nothing refers to yet.
    ///
    /// What they hold is the application's to bound. Carry each one
    /// on, write its reference, or abandon it.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn imports(&self) -> Result<Vec<Import>, Error> {
        let step = Step::new(self.store.view());
        let found = kept::imports(&step).await?;

        Ok(found.into_iter().map(import).collect())
    }

    /// How many bytes of a blob this device holds.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn blob_held(&self, blob: &BlobRef) -> Result<u64, Error> {
        let step = Step::new(self.store.view());
        let found = kept::find(&step, blob.listed()).await?;

        Ok(found.map_or(0, |(_, kept)| kept.bytes()))
    }

    /// Up to `length` bytes of a blob, from `offset`: fewer at its
    /// end, and never more than [`BLOB_STEP`]. `None` if this device
    /// doesn't hold all of that range.
    ///
    /// The bytes under a reference are the same wherever and whenever
    /// they're read.
    ///
    /// [`BLOB_STEP`]: Self::BLOB_STEP
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub async fn read_blob(
        &self,
        blob: &BlobRef,
        offset: u64,
        length: usize,
    ) -> Result<Option<Vec<u8>>, Error> {
        let step = Step::new(self.store.view());
        match kept::read(&step, blob.listed(), offset, length).await? {
            Bytes::Held(bytes) => Ok(Some(bytes)),
            Bytes::NotHeld => Ok(None),
        }
    }

    /// Deletes a step's worth of the bytes nothing refers to.
    pub(super) async fn sweep(&mut self, step: &mut Step<S::View>) -> Result<(), Error> {
        let swept = kept::sweep(step).await;
        if let Some(blob) = self.guard(swept)? {
            step.changed(Changed::Blob {
                name: blob.name,
                length: blob.length,
            });
        }

        Ok(())
    }
}
