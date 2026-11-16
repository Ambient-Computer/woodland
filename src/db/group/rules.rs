//! Holding committed rows to their table's rules.
//!
//! A rule is kept in its table's committed state. A transaction that
//! writes to a table with rules is tried before it's committed: if it
//! would leave a row that breaks one, it doesn't happen. Every device
//! commits the same transactions in the same order, so every device
//! turns down the same ones.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;

use crate::db::{GroupId, keys};
use crate::encoding;
use crate::model::table::{self, Filter, Place, Table};
use crate::model::{Kind, ModelId, Operation, Reads};
use crate::store::{StoreError, View};

use super::super::step::Step;
use super::deferred::Committed;

/// Whether any table these operations write to has a rule, so that
/// committing them has to be tried first.
pub(super) async fn bind<V: View>(
    group: GroupId,
    operations: &[Operation],
    step: &Step<V>,
) -> Result<bool, StoreError> {
    let tables = operations.iter().map(|it| it.model);
    let tables: BTreeSet<ModelId> = tables.filter(|it| it.kind() == Some(Kind::Table)).collect();
    for model in tables {
        let (start, end) = table::rules();
        let start = keys::committed_value(group, model, &start);
        let end = keys::committed_value(group, model, &end);
        if !step.scan(&start, &end).await?.is_empty() {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Whether every row under one of the `written` keys, as it's now
/// committed in `step`, meets every rule its table has.
pub(super) async fn hold<V: View>(
    group: GroupId,
    written: &[(ModelId, Vec<u8>)],
    dropped: &BTreeSet<ModelId>,
    step: &Step<V>,
) -> Result<bool, StoreError> {
    let mut rows: BTreeMap<ModelId, BTreeSet<Vec<u8>>> = BTreeMap::new();
    for (model, key) in written {
        let Some(Place::Row(row) | Place::Keys(row) | Place::Cell(row, ..)) = Place::parse(key)
        else {
            continue;
        };
        if model.kind() == Some(Kind::Table) {
            rows.entry(*model).or_default().insert(row);
        }
    }

    for (model, rows) in rows {
        let state = Committed {
            group,
            model,
            dropped,
            step,
        };
        let (start, end) = table::rules();
        let kept = state.scan(&start, &end).await?;
        let rules = kept
            .iter()
            .map(|(_, rule)| encoding::decode::<Filter>(rule));
        let Some(rules) = rules.collect::<Option<Vec<Filter>>>() else {
            return Ok(false);
        };
        if rules.is_empty() {
            continue;
        }

        let (start, end) = table::schema();
        let schema = state.scan(&start, &end.unwrap_or_default()).await?;
        for key in rows {
            let bound = || Bound::Included(key.clone());
            let (start, end) = table::rows(bound(), bound());
            let mut read = schema.clone();
            read.extend(state.scan(&start, &end.unwrap_or_default()).await?);
            let table = Table::read(&read);
            let row = table.as_ref().and_then(|table| table.row(&key));
            if row.is_some_and(|row| !rules.iter().all(|rule| rule.picks(row))) {
                return Ok(false);
            }
        }
    }

    Ok(true)
}
