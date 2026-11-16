//! Verdicts: which entries happen.
//!
//! An entry's operations can require something of a key. Each
//! requirement is checked against the state at the entry's place in the
//! timeline: what's committed, plus what every earlier uncommitted entry
//! that happened wrote. An entry with a requirement that doesn't hold is
//! pruned. It stays in its author's chain, but none of its operations
//! happen, in any model.
//!
//! A committed entry's verdict is final, because nothing can land before
//! it any more. An uncommitted entry's is provisional: a checkpoint, or a
//! late arrival, can change what came before it. So every uncommitted
//! verdict is worked out again whenever history changes.
//!
//! Nothing here sees a value. Operations declare what they write and
//! what they require, and that's all that's read.

use std::collections::{BTreeMap, BTreeSet};

use crate::clock::Stamp;
use crate::db::{Changed, GroupId, keys};
use crate::model::{Declaration, Effect, Fingerprint, Kind, ModelId, Operation, Requirement};
use crate::store::{StoreError, View};

use super::super::entry::Outline;
use super::super::step::Step;
use super::root::Accumulator;

/// A key in a model.
type Place = (ModelId, Vec<u8>);

/// What a key holds, as far as requirements can tell. `None` if nothing.
type Held = Option<Fingerprint>;

/// What uncommitted entries have done, partway along the timeline.
#[derive(Debug, Default)]
pub(super) struct Written {
    holds: BTreeMap<Place, Held>,
    /// Models dropped so far, committed drops included.
    dropped: BTreeSet<ModelId>,
}

impl Written {
    /// Nothing written yet, on top of committed state.
    pub(super) fn on(dropped: &BTreeSet<ModelId>) -> Self {
        Self {
            holds: BTreeMap::new(),
            dropped: dropped.clone(),
        }
    }

    pub(super) fn apply(&mut self, declared: &[Declaration]) {
        for declaration in declared {
            match &declaration.effect {
                Effect::Write { key, leaves } => {
                    self.holds.insert((declaration.model, key.clone()), *leaves);
                }

                // A drop does nothing until it's committed, and then
                // it's among the drops this started with.
                Effect::Drop | Effect::Require { .. } => {}
            }
        }
    }
}

/// Whether `entry` happens, given what came before it. Its own
/// operations apply in order, so it can meet a requirement itself, or
/// break one.
pub(super) async fn weigh<V: View>(
    id: GroupId,
    entry: &Outline,
    written: &Written,
    step: &Step<V>,
) -> Result<bool, StoreError> {
    let mut own: BTreeMap<Place, Held> = BTreeMap::new();
    let mut own_drops = BTreeSet::new();
    for declaration in &entry.declared {
        let model = declaration.model;
        if !declaration.belongs || written.dropped.contains(&model) || own_drops.contains(&model) {
            return Ok(false);
        }

        match &declaration.effect {
            Effect::Require { key, requirement } => {
                let place = (model, key.clone());
                let before = own.get(&place).or_else(|| written.holds.get(&place));
                let held = if let Some(&held) = before {
                    held
                } else {
                    let committed = step.get(&keys::committed_value(id, model, key)).await?;

                    committed.as_deref().map(Fingerprint::of)
                };

                let met = match requirement {
                    Requirement::Vacant => held.is_none(),
                    Requirement::Present => held.is_some(),
                    Requirement::Is(value) => held == Some(*value),
                };
                if !met {
                    return Ok(false);
                }
            }

            Effect::Write { key, leaves } => {
                own.insert((model, key.clone()), *leaves);
            }

            Effect::Drop => {
                own_drops.insert(model);
            }
        }
    }

    Ok(true)
}

/// Shows readers what an uncommitted entry wrote: a version of each key,
/// in order, so a later operation on the same key replaces an earlier
/// one. Not for models that only show what's committed. A drop leaves a
/// mark, and nothing else changes until it's committed.
pub(super) fn show<V>(id: GroupId, stamp: Stamp, operations: &[Operation], step: &mut Step<V>)
where
    V: View,
{
    // A merging model's key shows everything the transaction did to
    // it. Any other shows the last thing.
    let mut merged: BTreeMap<Place, Vec<Vec<u8>>> = BTreeMap::new();
    for operation in operations {
        let shown = operation.model.kind().is_some_and(Kind::shows_uncommitted);
        for write in operation.writes().into_iter().filter(|_| shown) {
            if operation.merges() {
                let writes = merged.entry((write.model, write.key)).or_default();
                writes.extend(write.value);
                continue;
            }

            step.save(
                keys::version(id, write.model, &write.key, stamp),
                &write.value,
            );
        }
    }
    for ((model, key), writes) in merged {
        let writes = Some(crate::encoding::encode(&writes));
        step.save(keys::version(id, model, &key, stamp), &writes);
    }
}

/// Takes back what [`show`] put there.
pub(super) fn hide<V: View>(id: GroupId, entry: &Outline, step: &mut Step<V>) {
    for declaration in &entry.declared {
        match &declaration.effect {
            Effect::Write { key, .. } => {
                step.delete(keys::version(id, declaration.model, key, entry.stamp()));
            }

            Effect::Drop | Effect::Require { .. } => {}
        }
    }
}

/// Makes an entry's writes the committed values of their keys, and
/// deletes every committed value of a model it drops. `state` is kept
/// in step: it's the sum of everything committed.
pub(super) async fn commit<V: View>(
    id: GroupId,
    operations: &[Operation],
    state: &mut Accumulator,
    step: &mut Step<V>,
) -> Result<(), StoreError> {
    for operation in operations {
        let model = operation.model;
        for write in operation.writes() {
            let value = match write.value {
                Some(value) if operation.merges() => {
                    let committed = keys::committed_value(id, model, &write.key);
                    let held = step.get(&committed).await?;

                    Some(crate::model::combine(held.as_deref(), &value))
                }

                value => value,
            };
            self::write(id, model, &write.key, value, state, step).await?;
        }

        if operation.drops() {
            let prefix = keys::committed_value(id, model, b"");
            let (start, end) = keys::range(prefix.clone());
            for (key, value) in step.scan(&start, &end).await? {
                state.clear(model, &key[prefix.len()..], &value);
                step.delete(key);
            }
            state.drop(model);
        }
    }

    Ok(())
}

/// Makes `value` the committed value of a key, or leaves it without
/// one.
pub(super) async fn write<V: View>(
    id: GroupId,
    model: ModelId,
    key: &[u8],
    value: Option<Vec<u8>>,
    state: &mut Accumulator,
    step: &mut Step<V>,
) -> Result<(), StoreError> {
    let committed = keys::committed_value(id, model, key);
    if let Some(old) = step.get(&committed).await? {
        state.clear(model, key, &old);
    }

    match value {
        Some(value) => {
            state.hold(model, key, &value);
            step.put(committed, value);
        }

        None => step.delete(committed),
    }

    Ok(())
}

/// Tells readers that whatever `entry` touches may read differently.
pub(super) fn notify<V: View>(id: GroupId, entry: &Outline, committed: bool, step: &mut Step<V>) {
    for &model in &entry.waits {
        step.changed(Changed::Pending { group: id, model });
    }

    for declaration in &entry.declared {
        match &declaration.effect {
            Effect::Write { key, .. } => step.changed(Changed::Key {
                group: id,
                model: declaration.model,
                key: key.clone(),
                committed,
            }),

            Effect::Drop if committed => step.changed(Changed::Everything { group: id }),
            Effect::Drop | Effect::Require { .. } => {}
        }
    }
}
