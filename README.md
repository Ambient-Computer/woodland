# Woodland

A peer-to-peer relational database for local-first apps.

> [!WARNING]
> Code written by AI. Early MVP. Breaking changes and no backwards compatibility.

## Purpose

At a high level, it's [CRDTs for SQL](https://blog.psychollama.io/crdts-theory-and-practice/).

In concrete terms:

- Developers can build apps without servers. No cost, uptime management, or data liability.
- Users own their data and can sync it between devices, without having to know it's any different.

Woodland handles the gnarly bits of mapping the relational abstraction to the messy world of concurrent edits and offline devices.

Build your app like it's on a normal relational database and it just works. Well, mostly works. You do have to learn a few things.

## Features

- **Tables:** Rows, typed cells, unique columns, foreign keys, rules, and local indexes.
- **Events:** Be notified when data changes. Re-run a query, sync to an external store, run an effect, etc.
- **Read levels:** Read `optimistic` or `committed`. Optimistic is the latest this device knows, and may be revised. Committed is the same on every device and never changes.
- **Transactions:** Atomic, with support for operations over committed state. Conflicts are handled deterministically, and the author is told the outcome.
- **Membership:** Writers (devices, services, peers) can be added and removed over time.
- **Compaction:** Any device can drop history, catch up from a snapshot, and restore history from a peer that kept it.
- **Untrusted Relay:** Operations are signed and verified, meaning updates can sync through untrusted peers.
- **Encryption:** Sealing means only approved devices can read it, allowing backups and relay from untrusted peers. (Note: metadata is not encrypted.)
- **Other Types:** Woodland's core is an operation log. Beside tables, it supports maps, claims, allowances, and ordered lists. All can apply in the same transaction.
- **Custom Types:** Apps can define custom models over the operation log. It's advanced and somewhat dangerous; avoid it unless you know what you're doing.

> [!NOTE]
> Woodland's queries are intentionally limited: bounded, ordered ranges. A full engine is too heavy for clients.
>
> If binary size doesn't matter, use a tool like [DataFusion](https://datafusion.apache.org/) or stream it and materialize to another database.

## Target Environments

Woodland is a Rust library written as a state machine. Consumers provide the business logic, I/O bindings, and the state machine driver. Networking assumes [Iroh](https://iroh.computer).

| Environment                      | Runs as                                     | Storage                                                                  |
| -------------------------------- | ------------------------------------------- | ------------------------------------------------------------------------ |
| Browser client                   | WebAssembly, in the main thread or a worker | [IndexedDB](https://developer.mozilla.org/en-US/docs/Web/API/IDBFactory) |
| Native app or self-hosted server | A Rust program (FFI embedding is planned)   | [redb](https://github.com/cberner/redb)                                  |
| Tests                            | Tests and simulations                       | Memory                                                                   |

A "server" is for backups, availability, and automated writes. It isn't a special role, just another peer.

## Caveats

- Woodland supports infinite read-only peers, but only a handful of writers.
- Checkpoints and some types of transactions need a weighted majority. If enough peers stay offline, committed bounds are stuck pending. (Woodland can heal these situations, but it's manual.)
- Sync obviously doesn't work if clients are offline. High availability needs at least one reachable replica. A "server" is just another peer.

## How it Works

Woodland is a [CRDT](https://github.com/alangibson/awesome-crdt) with consensus on top. The CRDT part handles writes across devices. The consensus part decides what's final.

Pieces:

- **Peer:** A device with a signing key.
- **Group:** A set of peers that share data. It tracks who can write and who can vote.
- **Clock:** One causal history inside a group. Peers subscribe to whole clocks, so it's how you'd split data wanted by different devices.
- **Model:** A table (or map, list, etc.) inside a clock.
- **Transaction:** A signed batch of writes to one clock.
- **Checkpoint:** A vote by a weighted majority of the group that makes a set of transactions final.

Flow:

1. A device writes a transaction. It shows up in optimistic reads right away.
2. Transactions sync whenever peers connect, either directly or through relays.
3. Every peer sorts transactions by causal time, so peers with the same transactions see the same state. If two transactions conflict, the same one loses on every peer.
4. When enough voters can reach each other, they run consensus on a checkpoint. Whatever it covers is committed, and shows up in committed reads.
5. Transactions that arrive later are ordered after the checkpoint. This is why an optimistic write can still lose to a committed one.

The full design is in [`docs/spec`](docs/spec/README.md), and the reasoning behind it in [`docs/context`](docs/context/README.md).
