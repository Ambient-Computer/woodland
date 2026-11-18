# Storage

Storage is an ordered key-value store that the [core](library.md) reads and writes. The contract is small so that it can be met everywhere the library runs, the browser included.

## Contract

- Keys and values are bytes. Keys are ordered bytewise.
- A batch of writes and deletes is applied atomically: all of it, or none.
- A batch is durable when applying it completes.
- A key is read by point lookup, and a range of keys by an ordered scan in either direction, a bounded number at a time.
- A reader can hold a consistent view: every batch applied before it, none after. A view is held briefly, since a store may make batches wait for it.
- A call may answer at once or later. The core [waits](library.md) for it either way.

## Drivers

- Stores are part of the library, not supplied by the application. There is one driver per kind of place the library runs, and an application chooses among them.
- A driver is included only when asked for, so an application carries only the stores it uses.
- Every driver passes the same tests of the contract, including being stopped mid-batch.
- Nothing above a driver knows which one it is on. Behaviour that differs between drivers is a defect in one of them.
- An in-memory driver exists for tests and simulation, and is available to applications for theirs.
- Owning the drivers is what lets the contract stay small: nothing has to be promised to code the library does not control.
- A delete frees space for the store to use again, and need not return it to the device. Every driver says how much it holds freed and unreturned, and returns it when the application asks. No [step](library.md) runs meanwhile.

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
- The transport signs with the same key, and is handed it from the store each time the device starts. A copy kept anywhere else can outlive the store, and whoever used it would be the peer that forgot.
- A device whose store is lost, evicted, or reset returns as a new [peer](peers.md) and is admitted again.
- Restoring a store from a backup is amnesia: the copy has forgotten everything since it was taken. A restored copy never resumes as the peer it was.
- Data is backed up by another peer holding it, not by copying a store.
- The old key of a device that returned as a new peer is [removed](membership.md), since it will never speak again.

## Layout

- Storage holds committed state, uncommitted history, the checkpoint chain, protocol promises, and whatever derived structures the peer keeps.
- What a device must read to start is bounded by uncommitted history; see [compaction](compaction.md).
- Derived structures can be dropped and rebuilt from the rest. Nothing else can.
- One store may hold many [groups](groups.md). A group's data is removable as a whole.
- [Blobs](blobs.md) and their imports are kept in the same store and written in the same batches, outside any group. Removing a group lets go of the blobs nothing else refers to.
- Only a peer with no role in a group removes it. A member that did would return having forgotten what it signed.
