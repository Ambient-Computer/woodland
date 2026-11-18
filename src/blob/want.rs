//! Which blobs a device would fetch without being asked.
//!
//! A policy says which: all of them, those up to a size, or none. It
//! covers the blobs a read would find a reference to, so each place
//! one is written is noted as it's written, if the blob isn't all
//! here. A note is only somewhere to look. Whether a read still finds
//! the reference there is checked when it comes to fetching, and a
//! note that's out of date is dropped then.
//!
//! A device that can't read what refers to a blob goes by the lists
//! its entries come with instead.

use serde::{Deserialize, Serialize};

use super::keys::{self, Place};
use super::{Listed, kept};
use crate::db::keys::{self as held, Record, Referrer};
use crate::db::step::Step;
use crate::db::{GroupId, entry::Entry};
use crate::encoding;
use crate::model::{self, ModelId};
use crate::store::{StoreError, View};

/// Which blobs a device fetches without being asked.
///
/// There's no default. A device says which before it holds any group,
/// with [`set_blob_policy`], and may say otherwise for any model.
///
/// It's this device's alone: nothing is agreed, and the devices of
/// one group differ. A group that wants its blobs kept has a device
/// that fetches every one.
///
/// [`set_blob_policy`]: crate::Database::set_blob_policy
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlobPolicy {
    /// Every one.
    All,
    /// Those of this many bytes or fewer.
    UpTo(u64),
    /// None. A blob is fetched when the application [asks] for it.
    ///
    /// [asks]: crate::Database::fetch_blob
    None,
}

impl BlobPolicy {
    pub(crate) fn covers(self, blob: Listed) -> bool {
        match self {
            Self::All => true,
            Self::UpTo(most) => blob.length <= most,
            Self::None => false,
        }
    }
}

/// Notes that `blob` may be found at `key`, unless it's all here or
/// the application let it go.
async fn consider<V: View>(
    step: &mut Step<V>,
    blob: Listed,
    key: Vec<u8>,
) -> Result<bool, StoreError> {
    let released = step.get(&keys::released(blob)).await?.is_some();
    if released || kept::is_whole(step, blob).await? {
        return Ok(false);
    }

    step.put(key, Vec::new());

    Ok(true)
}

/// Notes everywhere `step` wrote a reference to a blob that isn't all
/// here. `reads` is whether this device can read a group. Whether it
/// noted anything.
pub(crate) async fn note<V: View>(
    step: &mut Step<V>,
    reads: impl Fn(GroupId) -> bool,
) -> Result<bool, StoreError> {
    let wanted = |key: &[u8]| match held::parse(key) {
        Some(Record::Committed { model, key, .. } | Record::Version { model, key, .. }) => {
            model::may_refer(model, &key)
        }
        _ => matches!(
            held::referrer(key),
            Some((_, Referrer::Entry | Referrer::Carried))
        ),
    };
    let mut noted = false;
    for (stored, value) in step.written(&held::groups(), wanted) {
        let Some(value) = value else {
            continue;
        };

        match held::parse(&stored) {
            Some(Record::Committed { group, model, key }) => {
                if let Some(blob) = model::blob_under(model, &key, &value) {
                    let blob = blob.listed();
                    let at = keys::want_at(blob, group, model, &key);
                    noted |= consider(step, blob, at).await?;
                }
            }
            Some(Record::Version {
                group, model, key, ..
            }) => {
                let value = encoding::decode::<Option<Vec<u8>>>(&value).flatten();
                let blob = value.and_then(|value| model::blob_under(model, &key, &value));
                if let Some(blob) = blob {
                    let blob = blob.listed();
                    let at = keys::want_at(blob, group, model, &key);
                    noted |= consider(step, blob, at).await?;
                }
            }
            _ => {
                let (group, listed) = match held::referrer(&stored) {
                    Some((group, Referrer::Entry)) => {
                        let entry = encoding::decode::<Entry>(&value);

                        (group, entry.map(|it| it.blobs).unwrap_or_default())
                    }
                    Some((group, Referrer::Carried)) => (group, crate::db::carried(&value)),
                    _ => continue,
                };
                if reads(group) {
                    continue;
                }

                for blob in listed {
                    noted |= consider(step, blob, keys::want_listed(blob, group)).await?;
                }
            }
        }
    }

    Ok(noted)
}

/// How many versions of a key are looked through for a reference.
const VERSIONS: usize = 32;

/// Whether a read would still find a reference to `blob` at `place`.
pub(crate) async fn stands<V: View>(
    step: &Step<V>,
    blob: Listed,
    group: GroupId,
    place: &Place,
    reads: bool,
) -> Result<bool, StoreError> {
    let Place::At { model, key } = place else {
        let listed = step.get(&keys::refs(group, blob)).await?.is_some();

        return Ok(listed && !reads);
    };

    let refers = |value: &[u8]| {
        let found = model::blob_under(*model, key, value);

        found.is_some_and(|found| found.listed() == blob)
    };
    let committed = step.get(&held::committed_value(group, *model, key)).await?;
    if committed.is_some_and(|value| refers(&value)) {
        return Ok(true);
    }

    let (start, end) = held::range(held::versions(group, *model, key));
    for (stored, value) in step.scan_page(&start, &end, VERSIONS).await? {
        let ours =
            matches!(held::parse(&stored), Some(Record::Version { key: of, .. }) if of == *key);
        let value = encoding::decode::<Option<Vec<u8>>>(&value).flatten();
        if ours && value.is_some_and(|value| refers(&value)) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Drops every note of where `blob` may be found, and that it was
/// asked for: it's all here, or nothing refers to it.
pub(crate) async fn settle<V: View>(step: &mut Step<V>, blob: Listed) -> Result<(), StoreError> {
    let (start, end) = held::range(keys::wanted(blob));
    for (key, _) in step.scan(&start, &end).await? {
        step.delete(key);
    }
    step.delete(keys::asked(blob));

    Ok(())
}

/// The policy a model's blobs are fetched by, if it isn't the
/// device's.
pub(crate) async fn policy_of<V: View>(
    step: &Step<V>,
    model: ModelId,
) -> Result<Option<BlobPolicy>, StoreError> {
    step.load(&keys::policy_of(model)).await
}
