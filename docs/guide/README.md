# Using Woodland

How the library is meant to be used by an application. Rustdoc has the signatures, [`docs/spec`](../spec/README.md) has the guarantees, and [`examples`](../../examples) run. This page is the shape of an integration and what's expected of whoever writes one.

## The shape

- A `Database` is one device: every group it holds, over one `Store`.
- It is a state machine. It starts no threads, opens no connections, and reads no clock.
- Every call that changes anything is one step: one atomic write to the store, then `Outputs` saying what the application should do about it.
- The application owns everything else: the transport, the timer, the executor, and randomness.

## What the application provides

- **A store.** `FileStore` (feature `file`) for native, `BrowserStore` (feature `browser`) for wasm, `MemoryStore` for tests. One `Database` per store at a time.
- **A device secret.** 32 random bytes, passed to `Database::open`. Used only if the store is new; after that the store's own key wins. A device that loses its store is a different peer.
- Pass fresh random bytes on every start, and give the transport `device_secret()`, which is the key the store holds. Keep it nowhere else: a copy that outlives the store is a peer that forgot what it signed, and data in a group goes to whoever holds the group.
- **Seeds and nonces.** `found` takes a random seed. Model, column, and clock IDs take a nonce: random, or a constant every device derives alike, which is how devices agree on a name without talking.
- **A transport.** Anything that carries bytes between two peers known by their `VerifyingKey`. Woodland signs and checks everything; the transport is trusted with nothing but delivery.
- **An executor.** Futures are not `Send`, since a browser store can't be. On native, run the database on a local task set or its own thread.
- **A timer.** See [Time](#time).

## The loop

- On a connection opening: `connected(peer)`. On it closing: `disconnected(peer)`.
- On bytes arriving: `receive(peer, bytes)`, or `receive_all` for a batch, which is one store write instead of many.
- On a timer: `tick()`.
- After every call, handle its `Outputs`:
  - `sends`: give each message to the transport. A message for a peer that can't be reached is dropped; the protocol recovers on reconnect.
  - `changed`: re-run whatever reads it affects. See [Reading](#reading).
  - `outcomes`: what became of this device's own transactions.
  - `reconfigured`: what became of this device's requests to change a group's members.
  - `unfinished`: call `resume()` until it's false. A step does a bounded amount of work so none blocks for long.
- `Outputs` is given only after the step is durable. Send after, never before.
- `Error::Store` or `Error::Broken` means memory may be ahead of the store. Drop the `Database` and open it again.
- `examples/support/mod.rs` is this loop over an in-memory network, in about a hundred lines.

## Time

- `tick()` takes no time and reads none. It means "some time has passed", and nothing about correctness depends on when or how often it's called.
- It is when this device attempts a checkpoint, retries one that stalled, and stops assuming that what it sent its peers arrived.
- Without ticks, writes still replicate and show in optimistic reads. They are not committed.
- Tick on a coarse timer: every second or few while there is activity is plenty. Faster commits sooner and talks more.
- A device that holds a quorum by itself, such as a founder alone, commits on its own tick. Call `tick()` right after `write` there and nothing is ever left provisional. When to spend a checkpoint is the application's call, so `write` never does it unasked.

## Groups and peers

- `found(seed)` starts a group with this device as its only contributor. `found_sealed` starts one only its members can read.
- `invite(group)` gives bytes for another device to `join`. Pass them over a channel the joiner trusts: an invitation is believed.
- A device that joins holds and relays at once. It writes once the group gives it a role.
- `reconfigure(group, roles)` sets every role from the next checkpoint on. Anyone left out is removed. It takes effect when a checkpoint carries it.
- `configuration(group)` says who holds which role, by the latest checkpoint this device holds, and what this device has asked for that still stands. Build every `reconfigure` from it: keep no list of members of your own.
- `Changed::Members` says when to read it again. It is also how a device learns it was given a role.
- `Outputs::reconfigured` says what became of a request: `Carried`, or `Spent` because the group changed some other way first. A spent request did nothing. Read the configuration, and ask again if what was wanted still is.
- Roles: `Role::contributor(weight)` writes and votes, `Role::witness(weight)` votes only, `Role::reader(key)` reads a sealed group. `.only(clocks)` narrows a contributor; `.sealed_to(key)` gives a contributor a sealed group's secret.
- A quorum is counted by weight: see [checkpoints](../spec/checkpoints.md). Offline devices with weight delay commits; they never endanger them. Remove one that's gone for good.
- `want` holds only some clocks, `ask` narrows one connection, `forget` drops a group the device has no role in.

## Writing

- Build a `Transaction`, then `write(group, transaction)`. It's all or nothing, and readable here at once.
- A transaction writes to the models of one clock.
- `ModelId::kv`, `claims`, `threshold`, `allowances`, `sequence`, and `table` name the built-in kinds. `extend` adds an application's own, before any database is opened.
- Guards (`expect`, `expect_absent`, `expect_unchanged`) are how a write depends on a read. A transaction whose guard fails does nothing, on every device alike.
- Some operations do nothing until they are committed: `delete_where`, `update_where`, `delete_with`, `insert_numbered`, `add_rule`, and drops. Everything else in the same transaction shows at once. `pending` lists what is waiting.
- A sequence that orders rows keeps a row's key after such a delete removes the row. Skip keys with no row when reading, and remove them once the delete commits.
- Success is provisional until committed. `Outputs::outcomes` reports `Live`, `Pruned`, then one of `Committed`, `Rejected`, `Discarded`, or `Covered`.
- Outcomes are told once. Each names its group. After a restart, `outcome(id)` gives the last one, until `compact` sheds the transaction.

## Reading

- `read(&query)` answers from local state. There are two levels:
  - Optimistic (the default): everything this device knows, including its own uncommitted writes. May be undone.
  - Committed (`query.committed()`): exactly one checkpoint's state, final, and the answer says which checkpoint.
- A decision that must not be undone is made from committed reads, or waits for its transaction's `Committed`.
- Queries are bounded ordered ranges: a key, a prefix, a table, a range of rows, or an index range. There are no joins or aggregates; compute those in the application.
- `read_some(&query, limit)` reads a part at a time and returns the query for the rest.
- Readers such as `Table::read`, `Sequence::read`, and `Allowances::read` turn an answer into something typed.
- To subscribe: keep the `Query`, and re-run it when `query.is_affected_by(&change)` for a change in `Outputs::changed`. A change says where to look, not what's there.
- Every read made between two steps is of one moment. Nothing is held across a step.
- To keep a copy elsewhere (a UI store, a search index): read the committed state once, then follow `changed`, re-reading the keys it names. `Changed::Checkpoint` marks a consistent point; `Changed::Everything` means read it all again. `examples/mirror.rs` does this.
- `Table::rules` lists the rules in force, and `Filter::tests` says what a filter asks. A rule is judged at commit, so check a row against them before writing it.
- `add_index` keeps a local index of one or more table columns. It's this device's own and fills over several steps; `has_index` says when it's complete.

## Blobs

- Feature `blobs`. Without it a device still carries references and counts them; it keeps no bytes, and none of what follows exists.
- A blob is bytes too large to be a value. A table cell holds a `BlobRef`: a name, a length, and in a sealed group a key. The bytes are held apart, by the devices that choose to.
- A reference describes nothing. A file's name, type, and folder are columns beside it, and a directory is a table.
- `set_blob_policy` says which blobs this device fetches unasked: `All`, `UpTo(bytes)`, or `None`. There is no default, and `found` and `join` fail until it's said. `set_model_blob_policy` says otherwise for one model.
- To write one: `begin_import` (`begin_sealed_import` with 32 random bytes, for a sealed group), `import` the bytes in order, at most `BLOB_STEP` a call, then `finish_import` gives the reference to write. `imports` lists what's unfinished or finished and unwritten after a restart; `abandon_import` drops one. Nothing else lets go of them.
- A device writes only references to blobs it holds whole. To pass on one it was sent, fetch it first.
- `blob_held` says how many bytes of a blob are here, and `Changed::Blob` when that changes. `read_blob` reads a range, or says it isn't held. Treat every blob as possibly absent: nothing promises anyone has it.
- `fetch_blob` asks for one the policy leaves out. `release_blob` lets its bytes go and keeps the reference.
- Blob bytes travel apart from everything else. When `Outputs::blobs` names a peer, call `next_blob_message(peer)` for one bounded message, send it, and call again once it has gone. Hand what arrives to `receive_blob`. Give these a stream of their own and a large blob never delays sync.
- Bytes go when nothing refers to them: after a committed delete, that is when `compact` sheds the history that did.
- A group that wants its blobs kept needs a device whose policy is `All`. It need not be able to read them.

## Upkeep

- `compact(group)` sheds committed history and keeps committed state. Reads don't change. A device that never compacts is an archive.
- `skip_ahead(group)` has a device that's far behind take committed state instead of history.
- `restore(group, clock)` gets shed history back from an archive.
- `replace_secret(group)` asks a sealed group for a new secret, for one that may have leaked.
- `unreturned()` says about how much space the store has freed and still holds; `reclaim()` gives it back. Deleting never shrinks a `FileStore`'s file by itself. `reclaim` can take as long as rewriting the store, and fails while a view of it is held: call it when idle.
- `FileStore::open_with_cache` sets how much of the file is kept in memory. `open` allows 64 MiB.
- `succeed(old, seed)` founds a new group from the committed state of one that has lost its quorum for good. That is a decision for people.

## What to expect

- Peers may be offline for months and write the whole time. Nothing here assumes otherwise.
- Peers need not all reach each other. Any peer relays for any other, including for sealed groups it can't read.
- Committed is final: the same on every device, never reordered, never undone.
- Storage is bounded by committed state plus uncommitted history, once a device compacts.
- The public API uses `ed25519_dalek::VerifyingKey` for peer identity. Convert through its 32 bytes if the transport's key type differs, and key maps of peers by them.
- Build dependencies optimized, even for tests: sealing and signing are many times slower without. `[profile.dev.package."*"] opt-level = 3` in the application's `Cargo.toml` does it.
