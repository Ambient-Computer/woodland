# Storage

Storage is an ordered key-value store the application provides and the [core](library.md) calls. The contract is small so that it can be met everywhere the library runs, the browser included.

## Contract

- Keys and values are bytes. Keys are ordered bytewise.
- A batch of writes and deletes is applied atomically: all of it, or none.
- A batch is durable when the call that applies it returns.
- A key is read by point lookup, and a range of keys by an ordered scan in either direction.
- A reader can hold a consistent view while later batches are applied.
- Every call is synchronous.

## What is not assumed

- No transactions beyond one batch, and no conditional writes.
- No concurrent writers. The core is the only one.
- No secondary indexes, no query language, no knowledge of what is stored.
- No bound on value size is relied on; large values are the core's to split.
- Anything richer is built by the core on top, so that it behaves identically on every store.

## Crashes

- A process may die between any two calls and inside any call.
- After a crash the store holds exactly the batches whose calls returned, and possibly the one in progress, whole.
- A store that can lose a returned batch, or tear one, is not a store. No protocol above it is safe.

## Amnesia

- A peer that loses stored state has forgotten promises it signed. Continuing under the same key can break [agreement](checkpoints.md) and fork its own [chains](log.md).
- A peer's signing key is therefore kept in the same store as its state. Losing one loses both.
- A device whose store is lost, evicted, or reset returns as a new [peer](peers.md) and is admitted again.
- Restoring a store from a backup is amnesia: the copy has forgotten everything since it was taken. A restored copy never resumes as the peer it was.
- Data is backed up by another peer holding it, not by copying a store.
- The old key of a device that returned as a new peer is [removed](membership.md), since it will never speak again.

## Layout

- Storage holds committed state, uncommitted history, the checkpoint chain, protocol promises, and whatever derived structures the peer keeps.
- What a device must read to start is bounded by uncommitted history; see [compaction](compaction.md).
- Derived structures can be dropped and rebuilt from the rest. Nothing else can.
- One store may hold many [groups](groups.md). A group's data is removable as a whole.
