//! What every store has to do, as tests any driver can be put through.
//!
//! A driver's own tests call [`holds`] with a way to make an empty
//! store. Behaviour that differs between drivers is a defect in one
//! of them, and this is where it shows.
//!
//! It names nothing but the contract, and waits the way the library
//! does, so the same file tests a store that answers at once and one
//! in a browser that can't.

use super::{Order, Store, View, Write};

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

async fn apply<S: Store>(store: &mut S, batch: Vec<Write>) {
    store.apply(batch).await.expect("applied");
}

async fn get<V: View>(view: &V, key: &[u8]) -> Option<Vec<u8>> {
    view.get(key).await.expect("read")
}

/// A store holding each key, under itself.
async fn filled<S: Store>(mut store: S, keys: &[&[u8]]) -> S {
    apply(&mut store, keys.iter().map(|key| put(key, key)).collect()).await;

    store
}

/// The first two keys of a scan.
async fn keys<S: Store>(store: &S, start: &[u8], end: Option<&[u8]>, order: Order) -> Vec<Vec<u8>> {
    let entries = store.view().scan(start, end, order, 2).await;

    entries
        .expect("scanned")
        .into_iter()
        .map(|(key, _)| key)
        .collect()
}

/// Puts a driver through the whole contract. `empty` makes a store
/// with nothing in it, apart from any other it made.
pub(crate) async fn holds<S: Store, F: Future<Output = S>>(empty: impl Fn() -> F) {
    a_batch_applies_in_order(empty().await).await;
    a_view_doesnt_see_what_came_after_it(empty().await).await;
    a_view_can_be_copied_and_outlive_later_batches(empty().await).await;
    a_scan_stops_at_its_limit_from_either_end(empty().await).await;
    a_scan_includes_its_start_and_not_its_end(empty().await).await;
    a_scan_of_a_backwards_range_is_empty(empty().await).await;
    keys_are_ordered_bytewise(empty().await).await;
    keys_and_values_are_any_bytes(empty().await).await;
    deleting_nothing_is_fine(empty().await).await;
}

async fn a_batch_applies_in_order<S: Store>(mut store: S) {
    let batch = vec![
        put(b"a", b"1"),
        put(b"a", b"2"),
        put(b"b", b"1"),
        delete(b"b"),
    ];

    apply(&mut store, batch).await;

    let view = store.view();
    assert_eq!(get(&view, b"a").await, Some(b"2".to_vec()));
    assert_eq!(get(&view, b"b").await, None);
}

async fn a_view_doesnt_see_what_came_after_it<S: Store>(store: S) {
    let mut store = filled(store, &[b"a"]).await;
    let before = store.view();

    apply(&mut store, vec![delete(b"a"), put(b"b", b"b")]).await;

    assert_eq!(get(&before, b"a").await, Some(b"a".to_vec()));
    assert_eq!(get(&before, b"b").await, None);
    assert_eq!(get(&store.view(), b"a").await, None);
}

async fn a_view_can_be_copied_and_outlive_later_batches<S: Store>(store: S) {
    let mut store = filled(store, &[b"a", b"b"]).await;
    let before = store.view();
    let copy = before.clone();
    drop(before);

    apply(&mut store, vec![delete(b"a")]).await;
    apply(&mut store, vec![delete(b"b")]).await;

    let seen = copy
        .scan(b"", None, Order::Ascending, 8)
        .await
        .expect("scanned");
    assert_eq!(seen.len(), 2);
}

async fn a_scan_stops_at_its_limit_from_either_end<S: Store>(store: S) {
    let store = filled(store, &[b"a", b"b", b"c"]).await;

    assert_eq!(
        keys(&store, b"", None, Order::Ascending).await,
        [b"a", b"b"]
    );
    assert_eq!(
        keys(&store, b"", None, Order::Descending).await,
        [b"c", b"b"]
    );
}

async fn a_scan_includes_its_start_and_not_its_end<S: Store>(store: S) {
    let store = filled(store, &[b"a", b"b", b"c"]).await;

    assert_eq!(
        keys(&store, b"b", Some(b"c"), Order::Ascending).await,
        [b"b"]
    );
    assert_eq!(
        keys(&store, b"a", Some(b"b"), Order::Descending).await,
        [b"a"]
    );
}

async fn a_scan_of_a_backwards_range_is_empty<S: Store>(store: S) {
    let store = filled(store, &[b"a", b"b", b"c"]).await;

    assert_eq!(
        keys(&store, b"c", Some(b"a"), Order::Ascending).await,
        NOTHING
    );
    assert_eq!(
        keys(&store, b"b", Some(b"b"), Order::Ascending).await,
        NOTHING
    );
    assert_eq!(
        keys(&store, b"c", Some(b"a"), Order::Descending).await,
        NOTHING
    );
}

/// A shorter key sorts before any key it starts, and bytes compare
/// unsigned.
async fn keys_are_ordered_bytewise<S: Store>(store: S) {
    let store = filled(store, &[&[0x80], &[0x7f, 0xff], &[0x7f], &[]]).await;
    let entries = store.view().scan(b"", None, Order::Ascending, 8).await;
    let found: Vec<_> = entries
        .expect("scanned")
        .into_iter()
        .map(|it| it.0)
        .collect();

    assert_eq!(found, [vec![], vec![0x7f], vec![0x7f, 0xff], vec![0x80]]);
}

async fn keys_and_values_are_any_bytes<S: Store>(mut store: S) {
    let key = [0, 255, 0, 10, 13];
    let value = vec![7; 100_000];
    apply(&mut store, vec![put(&key, &value), put(b"empty", b"")]).await;

    let view = store.view();
    assert_eq!(get(&view, &key).await, Some(value));
    assert_eq!(get(&view, b"empty").await, Some(Vec::new()));

    // Nothing under a key and a key with nothing under it are told
    // apart by a scan too, and by a view of how things were.
    apply(&mut store, vec![put(b"empty", b"full")]).await;
    let scanned = view.scan(b"empty", None, Order::Ascending, 1).await;
    assert_eq!(scanned.expect("scanned"), [(b"empty".to_vec(), Vec::new())]);
    assert_eq!(get(&view, b"empty").await, Some(Vec::new()));
}

async fn deleting_nothing_is_fine<S: Store>(mut store: S) {
    apply(&mut store, vec![delete(b"never there")]).await;
    apply(&mut store, Vec::new()).await;

    assert_eq!(get(&store.view(), b"never there").await, None);
}
