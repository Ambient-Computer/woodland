# Woodland Spec

Woodland is a replicated relational database for small groups of devices that are mostly offline. It has no central authority: the devices that may write agree among themselves on who they are and on what is final.

## Goals

- Writing never waits on another device.
- Every device converges on the same state from the same history, whatever order it arrived in.
- An application chooses, per read, between the latest state and state that can never change.
- Data is expressed in a small, stable set of relational operations; an application feature never needs a new one.
- Nothing grows without bound: any device can shed history safely, and get it back with proof.
- Everything that travels is verifiable by whoever receives it, so any device can relay for any other, [readable to it or not](confidentiality.md).
- Other kinds of data live beside tables, in the same history and the same transactions.
- An application is a composition of many independent [groups](groups.md).

## Constraints

- A device may be offline for months, and may write the whole time.
- Devices connect in any shape; two contributors may never meet directly.
- Nothing assumes an application stays open long enough to finish an exchange.
- The library runs on client devices, native and WebAssembly, and behaves identically on both.
- The library is a [deterministic core](library.md) with its own [storage](storage.md). The network and the clock belong to the application.

## Non-goals

- Open participation. Writers are few and named; see [membership](membership.md).
- Outvoting a dishonest majority. Quorums are sized for devices that fail, not devices that lie.
- Preventing a contributor from misbehaving. Misbehaviour is proven and the contributor removed; see [log](log.md).
- Serializable isolation for optimistic work. See [transactions](transactions.md).
- Running application code during replay. A transaction records effects, not statements.
- Wall-clock ordering. Time is whatever a contributor says it was.
- Compatibility with SQL's surface. The capabilities are relational; the deviations are listed in [tables](tables.md) and [commit operations](commit-operations.md).

## Pages

- [peers](peers.md): devices, identity, and the roles a device can hold.
- [groups](groups.md): the unit of membership and finality, and the clocks and models it owns.
- [log](log.md): signed chains of transactions, causality, the timeline, and fork proofs.
- [membership](membership.md): configurations, quorums, joining, and removal.
- [checkpoints](checkpoints.md): the chain that makes history final.
- [transactions](transactions.md): effects, guards, and verdicts.
- [reads](reads.md): optimistic and committed reads, and watching for change.
- [tables](tables.md): rows, cells, constraints, references, and indexes.
- [commit operations](commit-operations.md): operations that take effect only when final.
- [migrations](migrations.md): changing a schema while old devices keep writing.
- [models](models.md): kinds of data other than tables.
- [compaction](compaction.md): shedding history, snapshots, and provable restore.
- [sync](sync.md): what two peers exchange, and what is checked on arrival.
- [confidentiality](confidentiality.md): sealed groups, secrets, and what removal guarantees.
- [library](library.md): the deterministic core, steps, and what the application owns.
- [storage](storage.md): the contract a store meets, the drivers that meet it, and what losing a store means.
