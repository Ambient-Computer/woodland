//! The browser's store, in a browser.
//!
//! Run with `wasm-bindgen-test-runner` as the runner for
//! `wasm32-unknown-unknown`, and a browser's driver on the path:
//!
//! ```sh
//! cargo test --target wasm32-unknown-unknown --features browser --test browser
//! ```

#![cfg(all(target_arch = "wasm32", feature = "browser"))]

use std::cell::Cell;

use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use woodland::{BrowserStore, Database, ModelId, Order, Query, Store, Transaction, View, Write};

/// The same tests every store is put through.
#[path = "../src/store/contract.rs"]
mod contract;

wasm_bindgen_test_configure!(run_in_browser);

thread_local! {
    static MADE: Cell<u32> = const { Cell::new(0) };
}

/// A name no other store has had.
fn name() -> String {
    let made = MADE.get() + 1;
    MADE.set(made);

    format!("test {} {made}", js_sys::Date::now())
}

async fn open(name: &str) -> BrowserStore {
    BrowserStore::open(name).await.expect("opened")
}

/// Lets the browser get on with whatever it has to do.
async fn turn() {
    let later = js_sys::Promise::new(&mut |resolve, _| {
        let global = js_sys::global();
        let wait = js_sys::Reflect::get(&global, &"setTimeout".into()).expect("a timer");
        let wait: js_sys::Function = wait.into();
        wait.call1(&global, &resolve).expect("set");
    });
    JsFuture::from(later).await.expect("waited");
}

async fn all<V: View>(view: &V) -> Vec<Vec<u8>> {
    let entries = view.scan(b"", None, Order::Ascending, 100).await;

    entries
        .expect("scanned")
        .into_iter()
        .map(|it| it.0)
        .collect()
}

#[wasm_bindgen_test]
async fn it_does_what_every_store_does() {
    contract::holds(|| async { open(&name()).await }).await;
}

#[wasm_bindgen_test]
async fn what_was_applied_is_there_when_its_opened_again() {
    let name = name();
    let mut store = open(&name).await;
    let batch = vec![contract::put(b"a", b"1"), contract::put(b"b", b"2")];
    store.apply(batch).await.expect("applied");
    store
        .apply(vec![contract::delete(b"a")])
        .await
        .expect("applied");
    drop(store);
    turn().await;

    let store = open(&name).await;
    let view = store.view();
    assert_eq!(view.get(b"a").await.expect("read"), None);
    assert_eq!(view.get(b"b").await.expect("read"), Some(b"2".to_vec()));
}

#[wasm_bindgen_test]
async fn a_store_has_one_owner_at_a_time() {
    let name = name();
    let first = open(&name).await;
    assert!(BrowserStore::open(&name).await.is_err());
    assert!(BrowserStore::open(&self::name()).await.is_ok());

    drop(first);
    turn().await;
    assert!(BrowserStore::open(&name).await.is_ok());
}

/// A page of a range, as it was, while the range changes under it:
/// some of the page deleted, more put ahead of it, and all of that
/// more than once.
#[wasm_bindgen_test]
async fn a_view_pages_through_what_it_saw() {
    let mut store = open(&name()).await;
    let keys: Vec<Vec<u8>> = (10..40u8).map(|key| vec![key]).collect();
    let fill = keys.iter().map(|key| contract::put(key, key)).collect();
    store.apply(fill).await.expect("applied");
    let before = store.view();

    let gone = keys.iter().step_by(2).map(|key| contract::delete(key));
    store.apply(gone.collect()).await.expect("applied");
    let ahead: Vec<Write> = (0..10u8).map(|key| contract::put(&[key], b"new")).collect();
    store.apply(ahead).await.expect("applied");
    let changed = keys.iter().map(|key| contract::put(key, b"changed"));
    store.apply(changed.collect()).await.expect("applied");

    let page = before.scan(b"", None, Order::Ascending, 5).await;
    let page = page.expect("scanned");
    let first: Vec<_> = keys[..5]
        .iter()
        .map(|key| (key.clone(), key.clone()))
        .collect();
    assert_eq!(page, first);

    let page = before.scan(&[12], Some(&[30]), Order::Descending, 3).await;
    let found: Vec<_> = page.expect("scanned").into_iter().map(|it| it.0).collect();
    assert_eq!(found, [vec![29], vec![28], vec![27]]);

    assert_eq!(all(&before).await, keys);
    assert_eq!(all(&store.view()).await.len(), 40);
}

#[wasm_bindgen_test]
async fn a_database_comes_back_from_the_browser() {
    let name = name();
    let store = open(&name).await;
    let mut database = Database::open(store, [1; 32]).await.expect("opened");
    let (group, _) = database.found([7; 32]).await.expect("founded");
    let notes = ModelId::kv(group, [1; 32]);
    let write = Transaction::new().set(notes, "todo", "milk");
    let outputs = database.write(group, write).await.expect("written");
    assert!(outputs.sends.is_empty(), "nobody to tell");
    let id = database.id();
    drop(database);
    turn().await;

    let database = Database::open(open(&name).await, [9; 32]).await;
    let database = database.expect("reopened");
    let read = database.read(&Query::key(group, notes, b"todo")).await;

    assert_eq!(database.id(), id, "the key is kept with the data");
    assert_eq!(read.expect("read"), [(b"todo".to_vec(), b"milk".to_vec())]);
}

/// What's been put ahead of a page since doesn't use up the page.
#[wasm_bindgen_test]
async fn a_page_isnt_shortened_by_what_came_later() {
    let mut store = open(&name()).await;
    let fill = (10..20u8).map(|key| contract::put(&[key], b"old"));
    store.apply(fill.collect()).await.expect("applied");
    let before = store.view();
    let ahead = (0..10u8).map(|key| contract::put(&[key], b"new"));
    store.apply(ahead.collect()).await.expect("applied");

    let page = before.scan(b"", None, Order::Ascending, 5).await;
    let found: Vec<_> = page.expect("scanned").into_iter().map(|it| it.0).collect();
    assert_eq!(found, [vec![10], vec![11], vec![12], vec![13], vec![14]]);

    let page = before.scan(b"", Some(&[15]), Order::Descending, 2).await;
    let found: Vec<_> = page.expect("scanned").into_iter().map(|it| it.0).collect();
    assert_eq!(found, [vec![14], vec![13]]);
}

/// A page can go away anywhere, and a step can be abandoned while it
/// waits. What's left of the batch it was applying is all of it or
/// none of it, and every batch before it.
///
/// Nothing in a page can kill the page, so this is as near as a test
/// gets: the batch is asked for, and then nobody waits for it.
#[wasm_bindgen_test]
async fn a_batch_abandoned_partway_is_there_whole_or_not_at_all() {
    use std::future::Future;
    use std::task::{Context, Waker};

    const WIDTH: u8 = 40;
    let name = name();
    let mut before = None;
    for round in 1..=4u8 {
        let mut store = open(&name).await;
        let batch = (0..WIDTH).map(|key| contract::put(&[key], &[round]));
        {
            let mut applying = std::pin::pin!(store.apply(batch.collect()));
            let mut context = Context::from_waker(Waker::noop());
            assert!(applying.as_mut().poll(&mut context).is_pending());
        }
        drop(store);
        turn().await;

        let store = open(&name).await;
        let view = store.view();
        let first = view.get(&[0]).await.expect("read");
        for key in 1..WIDTH {
            let held = view.get(&[key]).await.expect("read");
            assert_eq!(held, first, "round {round}: a batch was torn");
        }
        let whole = first == Some(vec![round]);
        assert!(whole || first == before, "round {round}: lost a batch");
        before = first;
        drop(view);
        drop(store);
        turn().await;
    }
}
