//! Committing a transaction that takes more than a step.
//!
//! Most transactions say what they write, and committing one is quick.
//! Two kinds aren't. One that has to be tried first, because it has an
//! operation to work out or a table's rules to meet, might take in a
//! whole table. One that drops a model takes every value the model
//! has out of committed state.
//!
//! Such a transaction is a job. It's done a part at a time, and where
//! it's got to is kept with its clock, so a device can stop anywhere
//! and carry on. A clock has one job at a time, for the transaction
//! that's next in its timeline, and commits nothing else until it's
//! done.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::clock::Stamp;
use crate::db::{Changed, keys};
use crate::encoding;
use crate::model::table::Cursor;
use crate::model::{Fingerprint, ModelId, Operation, Reads, Requirement};
use crate::store::{StoreError, View};

use super::super::entry::{Frontier, Outline};
use super::super::step::Step;
use super::clock::{Clock, Settled};
use super::deferred::{self, Committed, Tried};
use super::judge;
use super::root::Accumulator;
use super::rules;

/// About how many keys one part of a job looks at.
const PAGE: usize = if cfg!(test) { 6 } else { 256 };

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Job {
    /// The transaction.
    stamp: Stamp,
    phase: Phase,
    /// Models it drops, whose values are still committed.
    drops: BTreeSet<ModelId>,
    /// Models it dropped, whose values are gone.
    emptied: BTreeSet<ModelId>,
    happens: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Phase {
    /// Throwing away what an abandoned job left to one side.
    Clearing,
    /// Working out what its operations write, one after another, and
    /// setting that aside.
    Trying { operation: u32, at: Cursor },
    /// Holding what's been set aside to its tables' rules.
    Checking { after: Option<Vec<u8>> },
    /// It happens. Moving what's been set aside into committed state.
    Moving,
    /// Taking the values of the models it drops out of committed
    /// state.
    Emptying,
    /// It doesn't happen. Throwing away what's been set aside.
    Discarding,
}

impl Job {
    /// For a transaction that has to be tried before it's known
    /// whether it happens.
    pub(super) fn trying(stamp: Stamp) -> Self {
        Self {
            stamp,
            phase: Phase::Clearing,
            drops: BTreeSet::new(),
            emptied: BTreeSet::new(),
            happens: false,
        }
    }

    /// For a transaction that happens, whose writes are committed,
    /// and that drops these models.
    pub(super) fn emptying(stamp: Stamp, drops: BTreeSet<ModelId>) -> Self {
        Self {
            stamp,
            phase: Phase::Emptying,
            drops,
            emptied: BTreeSet::new(),
            happens: true,
        }
    }
}

/// A key of something set aside, as the model it's in and the model's
/// own key.
fn aside(prefix: &[u8], key: &[u8]) -> Option<(ModelId, Vec<u8>)> {
    let (model, key) = key.get(prefix.len()..)?.split_first_chunk::<64>()?;

    Some((ModelId::from_bytes(*model), key.to_vec()))
}

impl Clock {
    /// Commits what `frontier` covers, as far as `budget` goes. Returns
    /// whether that's all of it.
    pub(super) async fn work<V: View>(
        &mut self,
        frontier: &Frontier,
        budget: &mut usize,
        step: &mut Step<V>,
    ) -> Result<bool, StoreError> {
        loop {
            if self.job.is_some() {
                if !self.toil(budget, step).await? {
                    return Ok(false);
                }
                continue;
            }

            let (part, _, all) = self.history.part(frontier, *budget);
            let mut settled = self.settle(&part, step).await?;
            *budget = budget.saturating_sub(settled.verdicts.len());
            let through = std::mem::take(&mut settled.through);
            self.job = settled.job.take();
            self.commit(&through, settled, step);
            if self.job.is_none() && (all || *budget == 0) {
                return Ok(all);
            }
        }
    }

    /// Does as much of the job as `budget` goes to. Returns whether
    /// it's done, and its transaction committed.
    async fn toil<V: View>(
        &mut self,
        budget: &mut usize,
        step: &mut Step<V>,
    ) -> Result<bool, StoreError> {
        let Some(mut job) = self.job.take() else {
            return Ok(true);
        };
        let Some(entry) = self.history.uncommitted.get(&job.stamp).cloned() else {
            return Ok(true);
        };

        let stored = step.get(&keys::state(self.group, self.id)).await?;
        let stored = stored.and_then(|bytes| Accumulator::from_bytes(&bytes));
        let mut state = stored.unwrap_or_default();
        let mut done = false;
        let mut moved = Vec::new();
        while *budget > 0 && !done {
            *budget -= 1;
            done = self
                .turn(&mut job, &entry, &mut state, &mut moved, step)
                .await?;
        }
        // Verdicts on entries still to be committed go by what's
        // committed, which is changing under them.
        self.verdicts.moved(moved);
        step.put(keys::state(self.group, self.id), state.to_bytes());
        if !done {
            self.job = Some(job);
            return Ok(false);
        }

        let mut through = self.history.committed.clone();
        through.insert(entry.author, entry.link);
        let mut dropped = self.dropped.clone();
        dropped.extend(job.emptied);
        let settled = Settled {
            dropped,
            lost: false,
            verdicts: vec![(job.stamp, job.happens)],
            through: Frontier::new(),
            job: None,
        };
        self.commit(&through, settled, step);

        Ok(true)
    }

    /// Does one part of a job. Returns whether that was the last.
    async fn turn<V: View>(
        &self,
        job: &mut Job,
        entry: &Outline,
        state: &mut Accumulator,
        moved: &mut Vec<(ModelId, Vec<u8>)>,
        step: &mut Step<V>,
    ) -> Result<bool, StoreError> {
        let aside = keys::trial(self.group);
        let (first, end) = keys::range(aside.clone());
        match &mut job.phase {
            Phase::Clearing => {
                let page = step.scan_page(&first, &end, PAGE).await?;
                if page.len() < PAGE {
                    job.phase = Phase::Trying {
                        operation: 0,
                        at: Cursor::default(),
                    };
                }
                for (key, _) in page {
                    step.delete(key);
                }
            }

            Phase::Trying { .. } => {
                if !self.try_some(job, entry, step).await? {
                    job.phase = Phase::Discarding;
                }
            }

            Phase::Checking { after } => {
                let from = after.as_ref().map_or(first, |after| keys::after(after));
                let page = step.scan_page(&from, &end, PAGE).await?;
                let written = page.iter().filter_map(|(key, _)| self::aside(&aside, key));
                let written: Vec<_> = written.collect();
                let dropped: BTreeSet<ModelId> = self.dropped.union(&job.drops).copied().collect();
                if !rules::hold(self.group, &written, &dropped, step).await? {
                    job.phase = Phase::Discarding;
                } else if page.len() < PAGE {
                    job.phase = Phase::Moving;
                    job.happens = true;
                } else {
                    *after = page.last().map(|(key, _)| key.clone());
                }
            }

            Phase::Moving => {
                let page = step.scan_page(&first, &end, PAGE).await?;
                let last = page.len() < PAGE;
                for (at, stored) in page {
                    let tried = encoding::decode::<Tried>(&stored);
                    let place = self::aside(&aside, &at);
                    step.delete(at);
                    let (Some(tried), Some((model, key))) = (tried, place) else {
                        continue;
                    };

                    judge::write(self.group, model, &key, tried.value, state, step).await?;
                    moved.push((model, key.clone()));
                    step.changed(Changed::Key {
                        group: self.group,
                        model,
                        key,
                        committed: true,
                    });
                }
                if last {
                    job.phase = Phase::Emptying;
                }
            }

            Phase::Emptying => {
                let Some(&model) = job.drops.first() else {
                    return Ok(true);
                };

                let prefix = keys::committed_value(self.group, model, b"");
                let (start, end) = keys::range(prefix.clone());
                let page = step.scan_page(&start, &end, PAGE).await?;
                if page.len() < PAGE {
                    state.drop(model);
                    job.drops.remove(&model);
                    job.emptied.insert(model);
                }
                for (key, value) in page {
                    state.clear(model, &key[prefix.len()..], &value);
                    moved.push((model, key[prefix.len()..].to_vec()));
                    step.delete(key);
                }
            }

            Phase::Discarding => {
                let page = step.scan_page(&first, &end, PAGE).await?;
                let last = page.len() < PAGE;
                for (key, _) in page {
                    step.delete(key);
                }

                return Ok(last);
            }
        }

        Ok(false)
    }

    /// Works out some more of what the transaction writes, and sets it
    /// aside. Returns whether it can still happen.
    async fn try_some<V: View>(
        &self,
        job: &mut Job,
        entry: &Outline,
        step: &mut Step<V>,
    ) -> Result<bool, StoreError> {
        let Phase::Trying { operation: by, at } = &mut job.phase else {
            return Ok(true);
        };
        let operations = self.operations(entry, step).await?;
        let Some(operation) = operations.get(*by as usize) else {
            job.phase = Phase::Checking { after: None };
            return Ok(true);
        };

        let model = operation.model;
        let dropped: BTreeSet<ModelId> = self.dropped.union(&job.drops).copied().collect();
        let Some(waiting) = operation.waits() else {
            self.set_aside(operation, &dropped, step).await?;
            if operation.drops() {
                job.drops.insert(model);
            }
            (*by, *at) = (*by + 1, Cursor::default());

            return Ok(true);
        };
        if !operation.belongs() {
            return Ok(false);
        }

        // Each of its steps is checked against what the ones before
        // it left.
        let so_far = self.tried(model, &dropped, step);
        let Some(found) = waiting.resolve(&so_far, at, PAGE).await? else {
            return Ok(false);
        };
        for resolved in found.steps {
            let now = self.tried(model, &dropped, step).of(resolved.model);
            let Some(now) = now.filter(|_| !dropped.contains(&resolved.model)) else {
                return Ok(false);
            };

            for (key, requirement) in &resolved.requires {
                let held = now.get(key).await?;
                let held = held.as_deref().map(Fingerprint::of);
                let met = match requirement {
                    Requirement::Vacant => held.is_none(),
                    Requirement::Present => held.is_some(),
                    Requirement::Is(value) => held == Some(*value),
                };
                if !met {
                    return Ok(false);
                }
            }

            for (key, value) in resolved.writes {
                deferred::set_aside(self.group, resolved.model, &key, value, step);
            }
        }
        if let Some(next) = found.next {
            *at = next;
        } else {
            (*by, *at) = (*by + 1, Cursor::default());
        }

        Ok(true)
    }

    /// A model's committed state with what's been set aside laid over
    /// it.
    fn tried<'a, V: View>(
        &self,
        model: ModelId,
        dropped: &'a BTreeSet<ModelId>,
        step: &'a Step<V>,
    ) -> Committed<'a, V> {
        Committed {
            group: self.group,
            model,
            dropped,
            step,
        }
    }

    /// Sets aside what an operation that says what it writes, writes.
    async fn set_aside<V: View>(
        &self,
        operation: &Operation,
        dropped: &BTreeSet<ModelId>,
        step: &mut Step<V>,
    ) -> Result<(), StoreError> {
        let model = operation.model;
        for write in operation.writes() {
            let value = match write.value {
                Some(value) if operation.merges() => {
                    let now = self.tried(model, dropped, step);
                    let held = now.get(&write.key).await?;

                    Some(crate::model::combine(held.as_deref(), &value))
                }

                value => value,
            };
            deferred::set_aside(self.group, model, &write.key, value, step);
        }

        Ok(())
    }
}
