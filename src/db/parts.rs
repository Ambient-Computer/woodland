//! Reading the answer to a query a part at a time.

use crate::model::Query;
use crate::store::{Entry, Order, StoreError, View};

use super::{Found, keys, step};

/// How much of storage is read at once to find part of an answer.
const PAGE: usize = if cfg!(test) { 3 } else { 256 };

/// What `query` finds in `view`, up to `limit` of what it's of, and
/// the key the rest starts at if the limit is what stopped it.
pub(crate) async fn read<V: View>(
    view: &V,
    query: &Query,
    limit: usize,
) -> Result<(Vec<Found>, Option<Vec<u8>>), StoreError> {
    let mut found = Vec::new();
    if let Some(fixed) = query.fixed() {
        let mut scanned = Vec::new();
        for (start, end) in fixed.storage_ranges() {
            scanned.extend(step::scan(view, &start, &end).await?);
        }

        found = fixed.results(&scanned);
    }

    let mut from = query.first();
    let mut left = limit;
    loop {
        let (scanned, reached) = stretch(view, query, &from, limit.clamp(1, PAGE)).await?;
        let mut part = query.results(&scanned);
        let mut counted = (0..part.len()).filter(|&at| query.counts(&part[at].0));
        if let Some(over) = counted.nth(left) {
            let next = part[over].0.clone();
            part.truncate(over);
            found.extend(part);
            return Ok((found, Some(next)));
        }

        left -= part.iter().filter(|(key, _)| query.counts(key)).count();
        found.extend(part);
        match reached {
            Some(reached) if left > 0 => from = reached,
            reached => return Ok((found, reached)),
        }
    }
}

/// Reads on from `from` in what `query` asks for, about `size` of
/// storage's entries, and stops between two things: after a row, not
/// inside it. Gives what storage held up to there, and where that is.
/// Nowhere, if it's the end of what was asked for.
async fn stretch<V: View>(
    view: &V,
    query: &Query,
    from: &[u8],
    size: usize,
) -> Result<(Vec<Entry>, Option<Vec<u8>>), StoreError> {
    // Committed values and the versions over them are two ranges, and
    // each is known only as far as its page goes.
    let mut pages = Vec::new();
    let mut known: Option<Vec<u8>> = None;
    for (start, end) in query.span(from, None) {
        let page = view
            .scan(&start, Some(&end), Order::Ascending, size)
            .await?;
        let last = page.last().and_then(|(key, _)| Query::place(key));
        if let Some(last) = last.filter(|_| page.len() == size) {
            known = Some(known.map_or(last.clone(), |known| known.min(last)));
        }

        pages.push(page);
    }

    let Some(known) = known else {
        return Ok((pages.concat(), None));
    };

    // The thing the shorter page ends in is finished, from both.
    let reached = query.past(&known);
    let mut scanned = Vec::new();
    for (page, (_, end)) in pages.into_iter().zip(query.span(from, Some(&reached))) {
        let rest = page.last().map(|(key, _)| keys::after(key));
        let rest = rest.filter(|rest| page.len() == size && *rest < end);
        scanned.extend(page.into_iter().filter(|(key, _)| *key < end));
        if let Some(rest) = rest {
            scanned.extend(step::scan(view, &rest, &end).await?);
        }
    }

    Ok((scanned, (!query.is_past(&reached)).then_some(reached)))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::clock::Stamp;
    use crate::db::GroupId;
    use crate::encoding;
    use crate::model::ModelId;
    use crate::store::{MemoryStore, Store, Write};
    use crate::testing::block_on;
    use crate::testing::peers::{laptop, phone};

    fn group() -> GroupId {
        GroupId::from_bytes([1; 32])
    }

    fn model() -> ModelId {
        ModelId::kv(group(), [2; 32])
    }

    /// Keys that start one another, and hold the bytes that keys in
    /// storage are told apart by.
    fn key() -> impl Strategy<Value = Vec<u8>> {
        prop::collection::vec(prop::sample::select(vec![0, 1, b'a', 0xFF]), 0..4)
    }

    type Version = (Vec<u8>, u64, bool, Option<u8>);

    fn stored(committed: &[(Vec<u8>, u8)], versions: &[Version]) -> MemoryStore {
        let mut batch = Vec::new();
        for (key, value) in committed {
            let key = keys::committed_value(group(), model(), key);
            batch.push(Write::Put {
                key,
                value: vec![*value],
            });
        }

        for (key, lamport, by_phone, value) in versions {
            let author = if *by_phone { phone() } else { laptop() };
            let stamp = Stamp {
                lamport: *lamport,
                author,
            };
            let value = value.map(|value| vec![value]);
            batch.push(Write::Put {
                key: keys::version(group(), model(), key, stamp),
                value: encoding::encode(&value),
            });
        }

        let mut store = MemoryStore::new();
        block_on(store.apply(batch)).expect("stored");
        store
    }

    proptest! {
        /// Read a few at a time, an answer is the same as read at
        /// once, and every part but the last is as long as was asked
        /// for.
        #[test]
        fn parts_add_up_to_the_whole(
            committed in prop::collection::vec((key(), any::<u8>()), 0..24),
            versions in prop::collection::vec(
                (key(), 0..4u64, any::<bool>(), prop::option::of(any::<u8>())),
                0..24,
            ),
            prefix in key(),
            optimistic in any::<bool>(),
            limit in 1..6usize,
        ) {
            let store = stored(&committed, &versions);
            let view = store.view();
            let query = Query::prefix(group(), model(), &prefix);
            let query = if optimistic { query } else { query.committed() };

            let mut scanned = Vec::new();
            for (start, end) in query.storage_ranges() {
                scanned.extend(block_on(step::scan(&view, &start, &end)).expect("scanned"));
            }

            let whole = query.results(&scanned);
            let mut parts = Vec::new();
            let mut next = Some(query);
            while let Some(query) = next {
                let (found, rest) = block_on(read(&view, &query, limit)).expect("read");
                prop_assert!(found.len() <= limit);
                prop_assert!(rest.is_none() || found.len() == limit);
                prop_assert!(parts.len() <= whole.len());
                parts.extend(found);
                next = rest.map(|from| query.rest(from));
            }

            prop_assert_eq!(parts, whole);
        }
    }
}
