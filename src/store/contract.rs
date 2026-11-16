//! What every store has to do, as tests any driver can be put through.
//!
//! A driver's own tests call [`holds`] with a way to make an empty
//! store. Behaviour that differs between drivers is a defect in one
//! of them, and this is where it shows.

use super::{Order, Store, View, Write};
use crate::testing::block_on;

const NOTHING: [Vec<u8>; 0] = [];

pub(crate) fn put(key: &[u8], value: &[u8]) -> Write {
    Write::Put {
        key: key.to_vec(),
        value: value.to_vec(),
    }
}

pub(crate) fn delete(key: &[u8]) -> Write {
    Write::Delete { key: key.to_vec() }
}

pub(crate) fn apply<S: Store>(store: &mut S, batch: Vec<Write>) {
    block_on(store.apply(batch)).expect("applied");
}

pub(crate) fn get<V: View>(view: &V, key: &[u8]) -> Option<Vec<u8>> {
    block_on(view.get(key)).expect("read")
}

/// A store holding each key, under itself.
fn filled<S: Store>(mut store: S, keys: &[&[u8]]) -> S {
    apply(&mut store, keys.iter().map(|key| put(key, key)).collect());

    store
}

/// The first two keys of a scan.
fn keys<S: Store>(store: &S, start: &[u8], end: Option<&[u8]>, order: Order) -> Vec<Vec<u8>> {
    let entries = block_on(store.view().scan(start, end, order, 2));

    entries
        .expect("scanned")
        .into_iter()
        .map(|(key, _)| key)
        .collect()
}

/// Puts a driver through the whole contract. `empty` makes a store
/// with nothing in it, apart from any other it made.
pub(crate) fn holds<S: Store>(empty: impl Fn() -> S) {
    a_batch_applies_in_order(empty());
    a_view_doesnt_see_what_came_after_it(empty());
    a_view_can_be_copied_and_outlive_later_batches(empty());
    a_scan_stops_at_its_limit_from_either_end(empty());
    a_scan_includes_its_start_and_not_its_end(empty());
    a_scan_of_a_backwards_range_is_empty(empty());
    keys_are_ordered_bytewise(empty());
    keys_and_values_are_any_bytes(empty());
    deleting_nothing_is_fine(empty());
}

fn a_batch_applies_in_order<S: Store>(mut store: S) {
    let batch = vec![
        put(b"a", b"1"),
        put(b"a", b"2"),
        put(b"b", b"1"),
        delete(b"b"),
    ];

    apply(&mut store, batch);

    let view = store.view();
    assert_eq!(get(&view, b"a"), Some(b"2".to_vec()));
    assert_eq!(get(&view, b"b"), None);
}

fn a_view_doesnt_see_what_came_after_it<S: Store>(store: S) {
    let mut store = filled(store, &[b"a"]);
    let before = store.view();

    apply(&mut store, vec![delete(b"a"), put(b"b", b"b")]);

    assert_eq!(get(&before, b"a"), Some(b"a".to_vec()));
    assert_eq!(get(&before, b"b"), None);
    assert_eq!(get(&store.view(), b"a"), None);
}

fn a_view_can_be_copied_and_outlive_later_batches<S: Store>(store: S) {
    let mut store = filled(store, &[b"a", b"b"]);
    let before = store.view();
    let copy = before.clone();
    drop(before);

    apply(&mut store, vec![delete(b"a")]);
    apply(&mut store, vec![delete(b"b")]);

    let seen = block_on(copy.scan(b"", None, Order::Ascending, 8)).expect("scanned");
    assert_eq!(seen.len(), 2);
}

fn a_scan_stops_at_its_limit_from_either_end<S: Store>(store: S) {
    let store = filled(store, &[b"a", b"b", b"c"]);

    assert_eq!(keys(&store, b"", None, Order::Ascending), [b"a", b"b"]);
    assert_eq!(keys(&store, b"", None, Order::Descending), [b"c", b"b"]);
}

fn a_scan_includes_its_start_and_not_its_end<S: Store>(store: S) {
    let store = filled(store, &[b"a", b"b", b"c"]);

    assert_eq!(keys(&store, b"b", Some(b"c"), Order::Ascending), [b"b"]);
    assert_eq!(keys(&store, b"a", Some(b"b"), Order::Descending), [b"a"]);
}

fn a_scan_of_a_backwards_range_is_empty<S: Store>(store: S) {
    let store = filled(store, &[b"a", b"b", b"c"]);

    assert_eq!(keys(&store, b"c", Some(b"a"), Order::Ascending), NOTHING);
    assert_eq!(keys(&store, b"b", Some(b"b"), Order::Ascending), NOTHING);
    assert_eq!(keys(&store, b"c", Some(b"a"), Order::Descending), NOTHING);
}

/// A shorter key sorts before any key it starts, and bytes compare
/// unsigned.
fn keys_are_ordered_bytewise<S: Store>(store: S) {
    let store = filled(store, &[&[0x80], &[0x7f, 0xff], &[0x7f], &[]]);
    let entries = block_on(store.view().scan(b"", None, Order::Ascending, 8));
    let found: Vec<_> = entries
        .expect("scanned")
        .into_iter()
        .map(|it| it.0)
        .collect();

    assert_eq!(found, [vec![], vec![0x7f], vec![0x7f, 0xff], vec![0x80]]);
}

fn keys_and_values_are_any_bytes<S: Store>(mut store: S) {
    let key = [0, 255, 0, 10, 13];
    let value = vec![7; 100_000];
    apply(&mut store, vec![put(&key, &value), put(b"empty", b"")]);

    let view = store.view();
    assert_eq!(get(&view, &key), Some(value));
    assert_eq!(get(&view, b"empty"), Some(Vec::new()));
}

fn deleting_nothing_is_fine<S: Store>(mut store: S) {
    apply(&mut store, vec![delete(b"never there")]);
    apply(&mut store, Vec::new());

    assert_eq!(get(&store.view(), b"never there"), None);
}
