//! What a browser store held before recent batches, for views taken
//! before them.
//!
//! `IndexedDB` has no way to keep reading a moment that's passed: a
//! transaction that waits for anything else is closed. So a view reads
//! what the database holds now, and this puts back what batches have
//! changed since the view was taken. It's kept only while a view that
//! old exists, which is briefly.

use std::collections::BTreeMap;

use crate::store::{Entry, Order};

/// What some keys held before a batch. `None` for nothing.
pub(super) type Before = BTreeMap<Vec<u8>, Option<Vec<u8>>>;

#[derive(Debug, Default)]
pub(super) struct History {
    /// How many batches have been applied.
    generation: u64,
    /// How many views there are of each generation.
    held: BTreeMap<u64, usize>,
    /// What each batch replaced, by the generation it made.
    replaced: BTreeMap<u64, Before>,
}

impl History {
    /// Takes a view of the store as it is.
    pub(super) fn hold(&mut self) -> u64 {
        self.hold_at(self.generation)
    }

    /// Takes another view of the store as it was at `generation`.
    pub(super) fn hold_at(&mut self, generation: u64) -> u64 {
        *self.held.entry(generation).or_default() += 1;

        generation
    }

    /// Lets go of a view, and of whatever was kept only for it.
    pub(super) fn release(&mut self, generation: u64) {
        if let Some(count) = self.held.get_mut(&generation) {
            *count -= 1;
            if *count == 0 {
                self.held.remove(&generation);
            }
        }

        // A batch's old values matter to views from before it.
        let oldest = self.held.keys().next().copied();
        self.replaced
            .retain(|made, _| oldest.is_some_and(|oldest| oldest < *made));
    }

    /// Whether any view would miss what the next batch replaces.
    pub(super) fn watched(&self) -> bool {
        !self.held.is_empty()
    }

    /// Counts a batch as applied. `before` is what it replaced, which
    /// only matters if the store is [`watched`].
    ///
    /// [`watched`]: Self::watched
    pub(super) fn applied(&mut self, before: Before) {
        self.generation += 1;
        if self.watched() {
            self.replaced.insert(self.generation, before);
        }
    }

    /// What `key` held at `generation`, given what it holds `now`.
    pub(super) fn was(&self, generation: u64, key: &[u8], now: Option<Vec<u8>>) -> Option<Vec<u8>> {
        let since = self.replaced.range(generation + 1..);
        let mut held = since.filter_map(|(_, before)| before.get(key));

        // The first batch to write it replaced what the view saw.
        held.next().cloned().unwrap_or(now)
    }

    /// What every key in `start..end` held at `generation`, of those
    /// written since.
    pub(super) fn were(&self, generation: u64, start: &[u8], end: Option<&[u8]>) -> Before {
        let within = |key: &[u8]| start <= key && end.is_none_or(|end| key < end);
        let mut found = Before::new();
        for (_, before) in self.replaced.range(generation + 1..) {
            for (key, held) in before.iter().filter(|(key, _)| within(key)) {
                // The first batch to write a key replaced what the
                // view saw.
                found.entry(key.clone()).or_insert_with(|| held.clone());
            }
        }

        found
    }
}

/// The first `limit` entries of a range as a view saw it: `now`, read
/// from the database, with what's been replaced `since` put back.
/// `now` has to reach at least `limit` plus the size of `since` into
/// the range, if the range goes that far.
pub(super) fn restore(now: Vec<Entry>, since: Before, order: Order, limit: usize) -> Vec<Entry> {
    let mut entries: BTreeMap<Vec<u8>, Vec<u8>> = now.into_iter().collect();
    for (key, held) in since {
        match held {
            Some(value) => entries.insert(key, value),
            None => entries.remove(&key),
        };
    }

    match order {
        Order::Ascending => entries.into_iter().take(limit).collect(),
        Order::Descending => entries.into_iter().rev().take(limit).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn before(entries: &[(&str, Option<&str>)]) -> Before {
        let owned = |&(key, value): &(&str, Option<&str>)| (key.into(), value.map(Into::into));

        entries.iter().map(owned).collect()
    }

    fn entry(key: &str, value: &str) -> Entry {
        (key.into(), value.into())
    }

    #[test]
    fn a_view_reads_what_was_there_when_it_was_taken() {
        let mut history = History::default();
        let view = history.hold();
        history.applied(before(&[("a", Some("1")), ("b", None)]));
        history.applied(before(&[("a", Some("2")), ("c", Some("1"))]));
        let later = history.hold();

        let now = || Some(b"now".to_vec());
        assert_eq!(history.was(view, b"a", now()), Some(b"1".to_vec()));
        assert_eq!(history.was(view, b"b", now()), None);
        assert_eq!(history.was(view, b"z", now()), now());
        assert_eq!(history.was(later, b"a", now()), now());
        assert_eq!(
            history.were(view, b"a", Some(b"c")),
            before(&[("a", Some("1")), ("b", None)])
        );
        assert_eq!(history.were(view, b"b", None).len(), 2);
    }

    #[test]
    fn nothing_is_kept_that_no_view_needs() {
        let mut history = History::default();
        history.applied(before(&[("a", None)]));
        assert!(history.replaced.is_empty());

        let old = history.hold();
        let copy = history.hold_at(old);
        history.applied(before(&[("a", Some("1"))]));
        let new = history.hold();
        history.applied(before(&[("a", Some("2"))]));
        assert_eq!(history.replaced.len(), 2);

        history.release(old);
        assert_eq!(history.replaced.len(), 2);
        history.release(copy);
        assert_eq!(history.replaced.len(), 1);
        assert_eq!(history.was(new, b"a", None), Some(b"2".to_vec()));
        history.release(new);
        assert!(history.replaced.is_empty() && !history.watched());
    }

    #[test]
    fn a_page_is_filled_from_what_the_view_saw() {
        let now = vec![entry("b", "new"), entry("c", "1"), entry("d", "1")];
        let since = before(&[("a", Some("1")), ("b", Some("old")), ("c", None)]);

        assert_eq!(
            restore(now.clone(), since.clone(), Order::Ascending, 2),
            [entry("a", "1"), entry("b", "old")]
        );
        assert_eq!(
            restore(now, since, Order::Descending, 2),
            [entry("d", "1"), entry("b", "old")]
        );
    }
}
