use tempfile::TempDir;

use super::*;
use crate::store::contract::{self, apply, delete, get, put};
use crate::testing::block_on;
use crate::{Database, Query, Transaction};

fn folder() -> TempDir {
    tempfile::tempdir().expect("a folder")
}

fn open(folder: &TempDir, name: &str) -> FileStore {
    FileStore::open(folder.path().join(name)).expect("opened")
}

#[test]
fn it_does_what_every_store_does() {
    let folder = folder();
    let made = std::cell::Cell::new(0);

    contract::holds(|| {
        made.set(made.get() + 1);

        open(&folder, &made.get().to_string())
    });
}

#[test]
fn what_was_applied_is_there_when_its_opened_again() {
    let folder = folder();
    let mut store = open(&folder, "store");
    apply(&mut store, vec![put(b"kept", b"1"), put(b"gone", b"1")]);
    apply(&mut store, vec![delete(b"gone")]);
    drop(store);

    let view = open(&folder, "store").view();

    assert_eq!(get(&view, b"kept"), Some(b"1".to_vec()));
    assert_eq!(get(&view, b"gone"), None);
}

#[test]
fn a_file_has_one_owner_at_a_time() {
    let folder = folder();
    let first = open(&folder, "store");

    assert!(FileStore::open(folder.path().join("store")).is_err());

    drop(first);
    assert!(FileStore::open(folder.path().join("store")).is_ok());
}

#[test]
fn a_file_that_isnt_a_store_is_refused() {
    let folder = folder();
    let path = folder.path().join("notes.txt");
    std::fs::write(
        &path,
        "not a store, and long enough to be read as a header".repeat(64),
    )
    .expect("written");

    assert!(FileStore::open(&path).is_err());
}

/// The whole library on top of it: a device stops, and comes back as
/// itself with what it had.
#[test]
fn a_database_comes_back_from_its_file() {
    let folder = folder();
    let opened = block_on(Database::open(open(&folder, "store"), [1; 32]));
    let mut database = opened.expect("opened");
    let (group, _) = block_on(database.found([7; 32])).expect("founded");
    let notes = crate::ModelId::kv(group, [1; 32]);
    let written = block_on(database.write(group, Transaction::new().set(notes, "a", "b")));
    drop(written.expect("written"));
    let id = database.id();
    drop(database);

    let opened = block_on(Database::open(open(&folder, "store"), [2; 32]));
    let database = opened.expect("opened");
    let read = block_on(database.read(&Query::key(group, notes, b"a"))).expect("read");

    assert_eq!(database.id(), id, "the key is in the file");
    assert_eq!(read, [(b"a".to_vec(), b"b".to_vec())]);
}
