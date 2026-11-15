# Environments

Woodland runs inside other people's programs, on other people's devices. This page records what each place provides and what it takes away; the browser facts were checked in November 2026 and will age.

## Targets

- A web application, with the core compiled to WebAssembly.
- A host language calling the core through foreign-function bindings.
- A native Rust program, such as a daemon or a command-line tool.
- All three are expected to share one core. That is why it is a library that brings no runtime; see [library](../spec/library.md).
- A native program may block a thread on storage. A browser cannot block at all. The core is written once for both by awaiting.

## Transport

- Iroh is the expected transport. Its endpoint identifier is an Ed25519 public key, and so is a [peer](../spec/peers.md).
- The application supplies the secret key, so one key is the peer's identity in both layers.
- Iroh reached a stable 1.0 in June 2026. Its higher-level protocol crates had not, and Woodland uses none of them.
- A browser endpoint connects only through relay servers; it has no direct paths.
- Iroh's public relays are for development. A deployed application runs its own.
- A relay server is not a Woodland relay. It moves bytes between two endpoints; it holds nothing and is trusted with nothing.

## Storage by environment

- Native targets have real files and real fsync. Any embedded ordered store can sit under a driver.
- Browsers offer two places to keep data: IndexedDB, and the origin-private file system.
- IndexedDB is the browser driver. It is an ordered key-value store with atomic transactions, which is the [contract](../spec/storage.md) almost word for word.
- It works in every context a core might run in: a page, any kind of worker, a service worker. The core is not forced anywhere.
- A transaction opened with strict durability completes only after the platform has tried to reach the disk. Chrome's default is weaker, so the driver always asks.
- Its answers arrive later, never at once. That is why the core [awaits](../spec/library.md) storage rather than calling it.
- An IndexedDB transaction closes itself when its creator waits on anything but the database. A step waits only on storage, which is what keeps one transaction usable for a whole batch.
- Several tabs can write to one database. For Woodland that is a hazard and not a feature: two cores on one store are one peer twice.
- The driver holds a named lock for as long as it has the store, so a second tab is refused and routes through the first.

## Why not the origin-private file system

- Its only durable interface is synchronous, and exists only in a dedicated worker: not a shared worker, not a service worker, not the page.
- That would move the core into a worker, and the connection with it, and then every query, subscription, and action would cross a message port.
- It is a file, not a database. Atomicity and protection against corruption would be ours to build, on devices we cannot repair.
- A handle locks its file to one tab, so the multi-tab problem remains and is harder to share around.
- It is invisible to browser developer tools.
- It was the first choice here, made to keep storage synchronous. Awaiting storage removed the reason, and experience with both argued against it.
- It remains possible as a later driver if IndexedDB's read latency proves to matter.

## What the browser takes away

- Permanence, in principle. A browser may evict an origin's storage under pressure, and Safari may delete it after a week without use. Persistence can be requested.
- In practice this is rare on Chrome, desktop and Android, and for installed applications. How much to worry is the application's call, by platform.
- Presence. Background tabs are frozen or discarded within minutes, sooner on phones. A browser peer is rarely an always-on peer.
- Private windows keep nothing past the session.
- None of this stops a browser being a full peer: a contributor, with weight, holding everything. The library assumes neither that it is nor that it is not.
- What the library does require is that a browser that loses its store is treated as [amnesia](../spec/storage.md), like any other device that does.

## Keys

- The core signs synchronously and needs the secret key itself.
- Browser key stores that never reveal the key sign asynchronously and cannot serve as the transport identity, so they are not used.
- The key lives in the store beside the state it signed for, which is what the amnesia rule requires anyway.

## Sources

- File system access handles: MDN and the WHATWG File System standard.
- Storage eviction: MDN's storage quota page and WebKit's storage policy posts.
- Iroh: its documentation on browser support, deployment, and endpoint identity.
- IndexedDB durability and multi-tab behaviour: long use of both browser stores by the project's author, and the IndexedDB standard.
- Not verified: what strict durability does on each browser's disk, and whether a persistence grant exempts an origin from Safari's week limit.
