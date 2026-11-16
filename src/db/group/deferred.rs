//! Trying a transaction out, to one side.
//!
//! Most operations say what they write. A deferred one says what to
//! work out, and it's worked out once, at the transaction's place in
//! the committed timeline, against what every committed transaction
//! before it left. Whether the whole transaction can be done isn't
//! known until all of it has been tried, and it's all or nothing.
//!
//! So what it would write goes to one side, where nothing reads it but
//! the trying itself. That can take any number of steps: an operation
//! can take in a whole table. Once it's known to work, what was set
//! aside is moved into committed state. Every device that commits the
//! checkpoint does the same sum, and the checkpoint's state root says
//! what it came to, so whoever voted for the checkpoint agreed.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::db::{GroupId, keys};
use crate::model::{ModelId, Page, Reads};
use crate::store::{Entry, StoreError, View};

use super::super::step::Step;

/// What a transaction being tried would leave under a key.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Tried {
    pub(super) value: Option<Vec<u8>>,
}

/// A model's committed state as a step sees it, with what the
/// transaction being tried has set aside laid over it.
pub(super) struct Committed<'a, V> {
    pub(super) group: GroupId,
    pub(super) model: ModelId,
    pub(super) dropped: &'a BTreeSet<ModelId>,
    pub(super) step: &'a Step<V>,
}

impl<V: View> Committed<'_, V> {
    /// Lays what's been tried over `found`, which is everything
    /// committed between two keys of the model.
    fn overlay(&self, found: Vec<Entry>, tried: Vec<Entry>) -> Vec<Entry> {
        let committed = keys::committed_value(self.group, self.model, b"").len();
        let trial = keys::trial_value(self.group, self.model, b"").len();
        let bare = |(key, value): Entry| (key[committed..].to_vec(), value);
        let mut found: BTreeMap<Vec<u8>, Vec<u8>> = found.into_iter().map(bare).collect();
        for (key, stored) in tried {
            let key = key[trial..].to_vec();
            let seen = crate::encoding::decode::<Tried>(&stored);
            match seen.and_then(|tried| tried.value) {
                Some(value) => found.insert(key, value),
                None => found.remove(&key),
            };
        }

        found.into_iter().collect()
    }

    /// Where the keys in `start..end` are, under `prefix`.
    fn within(prefix: &[u8], start: &[u8], end: &[u8]) -> (Vec<u8>, Vec<u8>) {
        ([prefix, start].concat(), [prefix, end].concat())
    }
}

impl<V: View> Reads for Committed<'_, V> {
    fn model(&self) -> ModelId {
        self.model
    }

    fn of(&self, model: ModelId) -> Option<Self> {
        let Self {
            group,
            dropped,
            step,
            ..
        } = *self;
        let near = model.clock() == self.model.clock();

        near.then_some(Self {
            group,
            model,
            dropped,
            step,
        })
    }

    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        if self.dropped.contains(&self.model) {
            return Ok(None);
        }

        let tried = keys::trial_value(self.group, self.model, key);
        if let Some(tried) = self.step.load::<Tried>(&tried).await? {
            return Ok(tried.value);
        }

        let key = keys::committed_value(self.group, self.model, key);

        self.step.get(&key).await
    }

    async fn scan(&self, start: &[u8], end: &[u8]) -> Result<Vec<Entry>, StoreError> {
        if self.dropped.contains(&self.model) {
            return Ok(Vec::new());
        }

        let committed = keys::committed_value(self.group, self.model, b"");
        let (from, to) = Self::within(&committed, start, end);
        let found = self.step.scan(&from, &to).await?;
        let trial = keys::trial_value(self.group, self.model, b"");
        let (from, to) = Self::within(&trial, start, end);
        let tried = self.step.scan(&from, &to).await?;

        Ok(self.overlay(found, tried))
    }

    async fn page(&self, start: &[u8], end: &[u8], limit: usize) -> Result<Page, StoreError> {
        if self.dropped.contains(&self.model) {
            return Ok(Page::default());
        }

        let committed = keys::committed_value(self.group, self.model, b"");
        let (from, to) = Self::within(&committed, start, end);
        let mut found = self.step.scan_page(&from, &to, limit).await?;
        let trial = keys::trial_value(self.group, self.model, b"");
        let (from, to) = Self::within(&trial, start, end);
        let mut tried = self.step.scan_page(&from, &to, limit).await?;

        // Past the end of either full page, the other says nothing
        // certain.
        let last = |page: &[Entry], prefix: &[u8]| {
            let last = page.last().filter(|_| page.len() == limit);

            last.map(|(key, _)| key[prefix.len()..].to_vec())
        };
        let reach = match (last(&found, &committed), last(&tried, &trial)) {
            (Some(found), Some(tried)) => Some(found.min(tried)),
            (found, tried) => found.or(tried),
        };
        let near = |key: &[u8], prefix: &[u8]| {
            let key = &key[prefix.len()..];

            reach.as_ref().is_none_or(|reach| key <= reach.as_slice())
        };
        found.retain(|(key, _)| near(key, &committed));
        tried.retain(|(key, _)| near(key, &trial));

        Ok(Page {
            found: self.overlay(found, tried),
            reach,
        })
    }
}

/// Sets aside what the transaction being tried would leave under a
/// key.
pub(super) fn set_aside<V: View>(
    group: GroupId,
    model: ModelId,
    key: &[u8],
    value: Option<Vec<u8>>,
    step: &mut Step<V>,
) {
    step.save(keys::trial_value(group, model, key), &Tried { value });
}
