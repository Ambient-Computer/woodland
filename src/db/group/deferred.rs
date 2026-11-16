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

/// A model's committed state, as a step sees it.
struct Committed<'a, V> {
    group: GroupId,
    model: ModelId,
    step: &'a Step<V>,
}

impl<V: View> Reads for Committed<'_, V> {
    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        let key = keys::committed_value(self.group, self.model, key);

        self.step.get(&key).await
    }

    async fn scan(&self, start: &[u8], end: &[u8]) -> Result<Vec<Entry>, StoreError> {
        let prefix = keys::committed_value(self.group, self.model, b"");
        let within = |key: &[u8]| [&prefix[..], key].concat();
        let found = self.step.scan(&within(start), &within(end)).await?;
        let bare = |(key, value): Entry| (key[prefix.len()..].to_vec(), value);

        Ok(found.into_iter().map(bare).collect())
    }
}

/// Commits a transaction's operations in order, working out the ones
/// that waited as it gets to them. Returns the keys those wrote, or
/// `None` if one couldn't be done, and then `step` and `state` are
/// left partway: try it on a [fork](Step::fork) first.
pub(super) async fn carry_out<V: View>(
    group: GroupId,
    operations: &[Operation],
    dropped: &BTreeSet<ModelId>,
    state: &mut Accumulator,
    step: &mut Step<V>,
) -> Result<Option<Vec<(ModelId, Vec<u8>)>>, StoreError> {
    let mut touched = Vec::new();
    for operation in operations {
        let model = operation.model;
        let Some(deferred) = operation.waits() else {
            judge::commit(group, std::slice::from_ref(operation), state, step).await?;
            continue;
        };
        if !operation.belongs() || dropped.contains(&model) {
            return Ok(None);
        }

        let committed = Committed {
            group,
            model,
            step: &*step,
        };
        let Some(steps) = deferred.resolve(&committed).await? else {
            return Ok(None);
        };
        for resolved in steps {
            let committed = Committed {
                group,
                model,
                step: &*step,
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

    Ok(Some(touched))
}
