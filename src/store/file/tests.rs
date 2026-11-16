use tempfile::TempDir;

use super::*;
use crate::store::contract::{self, delete, put};
use crate::testing::block_on;
use crate::{Database, Query, Transaction};

fn apply(store: &mut FileStore, batch: Vec<Write>) {
    block_on(store.apply(batch)).expect("applied");
}

fn get(view: &FileView, key: &[u8]) -> Option<Vec<u8>> {
    block_on(view.get(key)).expect("read")
}

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

    block_on(contract::holds(|| {
        made.set(made.get() + 1);

        std::future::ready(open(&folder, &made.get().to_string()))
    }));
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
    assert_eq!(read.found(), [(b"a".to_vec(), b"b".to_vec())]);
}

/// How many keys each batch of the kill test writes.
const WIDTH: u8 = 40;

/// Set, for the process that gets killed, to the folder it writes in.
const VICTIM: &str = "WOODLAND_KILL_TEST_FOLDER";

fn number(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_be_bytes(bytes.try_into().ok()?))
}

/// Not a test of its own: [`a_store_killed_mid_batch_holds_whole_batches`]
/// runs it in another process, and kills it. Each batch writes its
/// number under every key, and once it's applied, says so in a file
/// beside the store.
#[test]
fn the_process_that_gets_killed() {
    let Some(folder) = std::env::var_os(VICTIM) else {
        return;
    };
    let folder = std::path::PathBuf::from(folder);
    let mut store = FileStore::open(folder.join("store")).expect("opened");
    let last = get(&store.view(), &[0]).and_then(|it| number(&it));
    for batch in last.map_or(1, |last| last + 1)..=u64::MAX {
        let value = batch.to_be_bytes();
        let writes = (0..WIDTH).map(|key| put(&[key], &value));
        apply(&mut store, writes.collect());
        std::fs::write(folder.join("applied.next"), value).expect("noted");
        std::fs::rename(folder.join("applied.next"), folder.join("applied")).expect("noted");
    }
}

/// A process can die anywhere. What's left is every batch whose call
/// returned, and maybe the one in progress, whole.
#[test]
fn a_store_killed_mid_batch_holds_whole_batches() {
    let folder = folder();
    let applied = || std::fs::read(folder.path().join("applied")).ok();
    let applied = || applied().and_then(|it| number(&it)).unwrap_or(0);
    let mut reached = 0;
    for round in 1..=4u64 {
        let test = "store::file::tests::the_process_that_gets_killed";
        let mut victim = std::process::Command::new(std::env::current_exe().expect("a test"))
            .args(["--exact", test, "--test-threads", "1"])
            .env(VICTIM, folder.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("started");

        // Let it get somewhere, a different distance each time.
        let patience = std::time::Instant::now();
        while applied() < reached + round * 3 {
            assert!(patience.elapsed().as_secs() < 60, "it never wrote");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        victim.kill().expect("killed");
        victim.wait().expect("gone");

        let returned = applied();
        let store = open(&folder, "store");
        let view = store.view();
        let held = |key: u8| get(&view, &[key]).and_then(|it| number(&it));
        let first = held(0).expect("something was applied");
        for key in 0..WIDTH {
            assert_eq!(held(key), Some(first), "round {round}: a batch was torn");
        }
        assert!(
            first >= returned,
            "round {round}: lost a batch that returned"
        );
        assert!(
            first <= returned + 1,
            "round {round}: holds a batch nobody applied"
        );
        reached = first;
    }
}
