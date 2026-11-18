//! Counting what refers to each blob.
//!
//! Three things a group holds can refer to a blob: a committed value,
//! a committed value kept while what replaces it is put in place, and
//! an entry, whatever became of it. A pruned transaction may be
//! revived, so its entry counts for as long as it's held.
//!
//! Nothing that writes those says so. Everything a step writes goes
//! through the step, so at its end the step's writes are compared with
//! what the store held before, and the counts follow. Nothing can
//! write a reference and forget to count it.
//!
//! An entry is counted by the list it comes with, by every device,
//! readable or not. A committed value is counted by what it is, which
//! only a device that can read it knows.
//!
//! The counts are derived: dropped, they're what going through
//! everything held again comes to.

use std::collections::{BTreeMap, BTreeSet};

use super::{Listed, keys};
use crate::db::keys::Referrer;
use crate::db::step::Step;
use crate::db::{GroupId, entry::Entry, keys as held};
use crate::encoding;
use crate::model;
use crate::store::{StoreError, View};

/// What a step's writes did to which blobs are referred to at all.
#[derive(Debug, Default)]
pub(crate) struct Tallied {
    /// Blobs nothing refers to any longer.
    pub(crate) freed: Vec<Listed>,
    /// Blobs something refers to that nothing did.
    pub(crate) revived: Vec<Listed>,
}

/// The blobs that what's under a key refers to.
fn referred(referrer: &Referrer, stored: Option<&[u8]>) -> Vec<Listed> {
    let Some(stored) = stored else {
        return Vec::new();
    };

    match referrer {
        Referrer::Entry => {
            let entry = encoding::decode::<Entry>(stored);

            entry.map(|it| it.blobs).unwrap_or_default()
        }
        Referrer::Value { model, key } => {
            let blob = model::blob_under(*model, key, stored);

            blob.iter().map(super::BlobRef::listed).collect()
        }
        Referrer::Kept { model, key } => {
            let kept = encoding::decode::<Option<Vec<u8>>>(stored).flatten();
            let blob = kept.and_then(|value| model::blob_under(*model, key, &value));

            blob.iter().map(super::BlobRef::listed).collect()
        }
    }
}

/// Whether what's under a key could refer to a blob, going by the key
/// alone. It saves reading what most keys held before.
fn could_refer(referrer: &Referrer) -> bool {
    match referrer {
        Referrer::Entry => true,
        Referrer::Value { model, key } | Referrer::Kept { model, key } => {
            model::may_refer(*model, key)
        }
    }
}

/// Brings the counts up to date with everything `step` has written.
/// It's the last thing a step does.
pub(crate) async fn tally<V: View>(step: &mut Step<V>) -> Result<Tallied, StoreError> {
    // A store that has never counted anything held nothing to stop
    // counting, which is every store that has no blobs.
    let (start, end) = held::range(keys::counts());
    let counted = !step.was_empty(&start, &end).await?;

    let mut changes: BTreeMap<(Listed, GroupId), i64> = BTreeMap::new();
    let wanted = |key: &[u8]| held::referrer(key).is_some_and(|(_, it)| could_refer(&it));
    for (key, now) in step.written(&held::groups(), wanted) {
        let Some((group, referrer)) = held::referrer(&key) else {
            continue;
        };

        for blob in referred(&referrer, now.as_deref()) {
            *changes.entry((blob, group)).or_default() += 1;
        }
        if counted {
            let was = step.was(&key).await?;
            for blob in referred(&referrer, was.as_deref()) {
                *changes.entry((blob, group)).or_default() -= 1;
            }
        }
    }

    changes.retain(|_, change| *change != 0);
    let touched: BTreeSet<Listed> = changes.keys().map(|(blob, _)| *blob).collect();
    let mut before = BTreeSet::new();
    for &blob in &touched {
        if is_referred(step, blob).await? {
            before.insert(blob);
        }
    }

    for ((blob, group), change) in changes {
        let key = keys::refs(group, blob);
        let count = step.load::<u64>(&key).await?.unwrap_or_default();
        let count = count.saturating_add_signed(change);
        if count == 0 {
            step.delete(key);
            step.delete(keys::referred_by(blob, group));
        } else {
            step.save(key, &count);
            step.put(keys::referred_by(blob, group), Vec::new());
        }
    }

    let mut tallied = Tallied::default();
    for blob in touched {
        match (before.contains(&blob), is_referred(step, blob).await?) {
            (true, false) => tallied.freed.push(blob),
            (false, true) => tallied.revived.push(blob),
            _ => {}
        }
    }

    Ok(tallied)
}

/// Whether anything any group holds refers to `blob`.
pub(crate) async fn is_referred<V: View>(step: &Step<V>, blob: Listed) -> Result<bool, StoreError> {
    let (start, end) = held::range(keys::referrers(blob));

    Ok(!step.scan(&start, &end).await?.is_empty())
}

/// What the counts would be if they were worked out again from
/// everything `view` holds, and what they're kept as.
#[cfg(test)]
pub(crate) async fn recount<V: View>(
    view: &V,
) -> Result<(BTreeMap<Vec<u8>, u64>, BTreeMap<Vec<u8>, u64>), StoreError> {
    let (start, end) = held::range(held::groups());
    let mut counted = BTreeMap::new();
    for (key, value) in crate::db::step::scan(view, &start, &end).await? {
        let Some((group, referrer)) = held::referrer(&key) else {
            continue;
        };

        for blob in referred(&referrer, Some(&value)) {
            *counted.entry(keys::refs(group, blob)).or_default() += 1;
        }
    }

    let (start, end) = held::range(keys::counts());
    let kept = crate::db::step::scan(view, &start, &end).await?;
    let kept = kept.into_iter().filter_map(|(key, count)| {
        let count = encoding::decode::<u64>(&count)?;

        Some((key, count))
    });

    Ok((counted, kept.collect()))
}
