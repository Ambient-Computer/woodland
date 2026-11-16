//! Carrying out a transaction with changes that waited to be committed.
//!
//! Most operations say what they write. A deferred one says what to
//! work out, and it's worked out here: once, at the transaction's
//! place in the committed timeline, against what every committed
//! transaction before it left. Every device that commits the
//! checkpoint does the same sum, and the checkpoint's state root says
//! what it came to, so whoever voted for the checkpoint agreed.

use std::collections::BTreeSet;

use crate::db::{GroupId, keys};
use crate::model::{Fingerprint, ModelId, Operation, Reads, Requirement};
use crate::store::{Entry, StoreError, View};

use super::super::step::Step;
use super::judge;
use super::root::Accumulator;
use super::rules;

/// A model's committed state, as a step sees it.
pub(super) struct Committed<'a, V> {
    pub(super) group: GroupId,
    pub(super) model: ModelId,
    pub(super) dropped: &'a BTreeSet<ModelId>,
    pub(super) step: &'a Step<V>,
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

        let key = keys::committed_value(self.group, self.model, key);

        self.step.get(&key).await
    }

    async fn scan(&self, start: &[u8], end: &[u8]) -> Result<Vec<Entry>, StoreError> {
        if self.dropped.contains(&self.model) {
            return Ok(Vec::new());
        }

        let prefix = keys::committed_value(self.group, self.model, b"");
        let within = |key: &[u8]| [&prefix[..], key].concat();
        let found = self.step.scan(&within(start), &within(end)).await?;
        let bare = |(key, value): Entry| (key[prefix.len()..].to_vec(), value);

        Ok(found.into_iter().map(bare).collect())
    }
}

/// Commits a transaction's operations in order, working out the ones
/// that waited as it gets to them. Returns the keys those wrote, or
/// `None` if one couldn't be done or a row was left breaking a rule,
/// and then `step` and `state` are left partway: try it on a
/// [fork](Step::fork) first.
pub(super) async fn carry_out<V: View>(
    group: GroupId,
    operations: &[Operation],
    dropped: &BTreeSet<ModelId>,
    state: &mut Accumulator,
    step: &mut Step<V>,
) -> Result<Option<Vec<(ModelId, Vec<u8>)>>, StoreError> {
    let mut touched = Vec::new();
    let mut written = Vec::new();
    for operation in operations {
        let model = operation.model;
        let Some(deferred) = operation.waits() else {
            let writes = operation.writes().into_iter();
            written.extend(writes.map(|write| (write.model, write.key)));
            judge::commit(group, std::slice::from_ref(operation), state, step).await?;
            continue;
        };
        if !operation.belongs() || dropped.contains(&model) {
            return Ok(None);
        }

        let committed = Committed {
            group,
            model,
            dropped,
            step: &*step,
        };
        let Some(steps) = deferred.resolve(&committed).await? else {
            return Ok(None);
        };
        for resolved in steps {
            // A change reaches other models only through `Reads::of`,
            // so a step for one that isn't near was never read.
            let own = Committed {
                group,
                model,
                dropped,
                step: &*step,
            };
            let model = resolved.model;
            let Some(committed) = own.of(model).filter(|_| !dropped.contains(&model)) else {
                return Ok(None);
            };

            for (key, requirement) in &resolved.requires {
                let held = committed.get(key).await?;
                let held = held.as_deref().map(Fingerprint::of);
                let met = match requirement {
                    Requirement::Vacant => held.is_none(),
                    Requirement::Present => held.is_some(),
                    Requirement::Is(value) => held == Some(*value),
                };
                if !met {
                    return Ok(None);
                }
            }

            for (key, value) in resolved.writes {
                judge::write(group, model, &key, value, state, step).await?;
                touched.push((model, key));
            }
        }
    }

    written.extend(touched.iter().cloned());
    if !rules::hold(group, &written, dropped, step).await? {
        return Ok(None);
    }

    Ok(Some(touched))
}
