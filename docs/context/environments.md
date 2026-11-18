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
- A transaction that has stopped answering cannot be read again, so a view that outlives a batch cannot be a transaction. The driver reads the present and puts back what batches replaced since the view was taken, kept only while such a view exists.
- Verified by running the driver's tests in Firefox 159 and Chromium 156, November 2026: keys of raw bytes order bytewise, the empty key included; a transaction asked for strict durability is accepted; a held lock refuses a second owner and is free again once released.
- Not verified: any of this in Safari, in a worker, or across two real tabs. The tests run in one page.
- Reported by the author of an application built on the library, November 2026, and not run here: the driver in a shared worker in Chromium 156, with a reload and a second tab attaching to the same worker.

## Stores looked at and not used

- Looked at in November 2026, from documentation and release notes. None was run.
- The native driver sits on redb: one file, written in Rust, with atomic batches and consistent views. A device with a large disk that keeps everything needs nothing more.
- Fjall is a log-structured store in Rust with atomic batches and views. It is the alternative if write volume ever strains the native driver.
- SlateDB is a log-structured store kept in object storage, with range reads and atomic batches. It is the candidate if an archive ever has to outgrow a disk.
- Not verified for SlateDB: how it keeps a second writer out, and how far its views had got. Sources disagreed.
- DynamoDB limits one atomic write to 100 items and 4 MB. A step's batch has no such limit, so a driver would need a log of its own on top.
- Object storage used directly has no ordered range that changes atomically. A driver on it would be a storage engine.
- Any remote store makes each of a step's many small reads a round trip, and needs something to guarantee one writer. Neither is solved here.

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

## Blobs: what was looked at

- Looked at in November 2026: the `iroh-blobs` crate at 0.105, from its source, changelog, and issues. It was read, not run.
- It is not used. Its own readme calls that line of releases not yet of production quality, and its latest release made seven breaking changes.
- Its store is an actor on a runtime of its own, with a database and files beside ours. That is a second store to keep in step, on devices we cannot repair.
- Its collector deleted blobs still in use until two fixes that month, and stops for good after one failed run. Both were open or newly closed issues at the time.
- It has no store that persists in a browser, no encryption, and by default serves a blob to anyone who knows its hash.
- What is taken from it is the format the [spec](../spec/blobs.md) names. Keeping its 16 KiB leaves an open blob's name the one `iroh-blobs` gives it.
- The `bao-tree` crate was to supply the tree, and was built for `wasm32-unknown-unknown` at 0.17. It is not used: it has not reached 1.0, and brought about two dozen crates.
- The `blake3` crate's own `hazmat` module hashes a subtree at an offset and joins two, which is all the tree needs. It was already a dependency.
- A sealed blob is named by the hash of its encrypted bytes, so a stock `iroh-blobs` node cannot be handed one by the name its readers know.
- Its collections were looked at for directories: a blob listing child hashes, with a second blob of names. Their documentation says the format may change, and it holds names only.

## Blobs: what was measured

- Measured in November 2026 on one laptop, on ZFS, at idle priority while the machine was short of memory. Rates are rough; ratios are not.
- The native driver was given blobs as values of one size under ordered keys, about 4 MiB a batch, up to 3 GiB.
- Its file was 2.13 times the bytes stored at 16 KiB a value, 1.53 at 48 KiB, 1.27 at 112 KiB, 1.13 at 240 KiB, and 1.07 at 496 KiB. One value per verified group wastes half the file.
- No batch took longer than half a second, and most took tens of milliseconds. Deleting 1 GiB a batch at a time took one to two seconds.
- Deleting did not shrink the file. Writing the same amount again fitted in the space freed.
- The native driver's memory peaked near 1.1 GiB, which is the cache redb takes unless told otherwise. That is too much for a client device and has nothing to do with blobs.
- The browser driver was given the same in headless Firefox, up to 256 MiB. Chromium and Safari were not run.
- A blob of 256 KiB took one batch of 11 ms. At 64 MiB it wrote 14 MiB a second at 16 KiB a value and 47 at 112 KiB.
- At 256 MiB its slowest 4 MiB batch took 1.4 to 1.8 seconds, and deleting took ten. The page is not blocked, but the core does nothing else in that time.
- A value of 496 KiB took 32 ms to read back in the browser, against 2 ms at 112 KiB.
- Values of 112 KiB, seven verified groups, were the best size measured on both. Batches smaller than 4 MiB were not measured in the browser.
- redb's own compaction returned the space: a file of 4.5 MiB with everything deleted shrank below a quarter, in a test that runs with the rest. Large stores were not timed.
- redb's cache is capped at 64 MiB by default since. That figure was chosen, not measured.
- Not tried: anything that returns freed space in a browser. IndexedDB offers no call for it and does not say what it has freed.
- Not measured: tens of gigabytes anywhere, and whether a browser's strict durability reaches as far for large values.
- Not tried: keeping pieces in the browser as its own `Blob` objects, which it stores as files and reads a slice at a time. They might read faster. Safari has a history of losing them, and whether strict durability covers them is not known.

## Sources

- File system access handles: MDN and the WHATWG File System standard.
- Storage eviction: MDN's storage quota page and WebKit's storage policy posts.
- Iroh: its documentation on browser support, deployment, and endpoint identity.
- Stores: the redb, Fjall, and SlateDB documentation, and the DynamoDB API reference for transactional writes.
- IndexedDB durability and multi-tab behaviour: long use of both browser stores by the project's author, and the IndexedDB standard.
- Not verified: what strict durability does on each browser's disk, and whether a persistence grant exempts an origin from Safari's week limit.
