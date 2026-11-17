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
- Success is provisional until committed. `Outputs::outcomes` reports `Live`, `Pruned`, then one of `Committed`, `Rejected`, `Discarded`, or `Covered`.
- Outcomes are told once. After a restart, `outcome(group, id)` gives the last one, until `compact` sheds the transaction.

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
- `add_index` keeps a local index of one or more table columns. It's this device's own and fills over several steps; `has_index` says when it's complete.

## Upkeep

- `compact(group)` sheds committed history and keeps committed state. Reads don't change. A device that never compacts is an archive.
- `skip_ahead(group)` has a device that's far behind take committed state instead of history.
- `restore(group, clock)` gets shed history back from an archive.
- `replace_secret(group)` asks a sealed group for a new secret, for one that may have leaked.
- `succeed(old, seed)` founds a new group from the committed state of one that has lost its quorum for good. That is a decision for people.

## What to expect

- Peers may be offline for months and write the whole time. Nothing here assumes otherwise.
- Peers need not all reach each other. Any peer relays for any other, including for sealed groups it can't read.
- Committed is final: the same on every device, never reordered, never undone.
- Storage is bounded by committed state plus uncommitted history, once a device compacts.
- The public API uses `ed25519_dalek::VerifyingKey` for peer identity. Convert through its 32 bytes if the transport's key type differs.
