# Environments

Woodland runs inside other people's programs, on other people's devices. This page records what each place provides and what it takes away; the browser facts were checked in November 2026 and will age.

## Targets

- A web application, with the core compiled to WebAssembly.
- A host language calling the core through foreign-function bindings.
- A native Rust program, such as a daemon or a command-line tool.
- All three are expected to share one core. That is why it is a library with no runtime; see [library](../spec/library.md).

## Transport

- Iroh is the expected transport. Its endpoint identifier is an Ed25519 public key, and so is a [peer](../spec/peers.md).
- The application supplies the secret key, so one key is the peer's identity in both layers.
- Iroh reached a stable 1.0 in June 2026. Its higher-level protocol crates had not, and Woodland uses none of them.
- A browser endpoint connects only through relay servers; it has no direct paths.
- Iroh's public relays are for development. A deployed application runs its own.
- A relay server is not a Woodland relay. It moves bytes between two endpoints; it holds nothing and is trusted with nothing.

## Storage by environment

- Native targets have real files and real fsync. Any embedded ordered store meets the [contract](../spec/storage.md).
- Browsers offer one synchronous, durable-ish primitive: sync access handles on the origin-private file system.
- Those handles exist only in a dedicated worker. A core with synchronous storage therefore lives in a worker, and the page talks to it by messages.
- A handle locks its file exclusively. One tab's worker owns the store; other tabs go through it. This matches the [one-owner rule](../spec/library.md).
- No pure-Rust ordered store was found that runs on those handles. The browser store is ours to write, or sits on SQLite's.
- IndexedDB is asynchronous. Calling it synchronously needs a WebAssembly feature that had only just reached every major browser, so it is a fallback and not the plan.

## What the browser takes away

- Durability. The platform's flush promises an attempt, not a result.
- Permanence. A browser may evict an origin's storage under pressure, and Safari may delete it after a week without use. Persistence can be requested and is not guaranteed.
- Presence. Background tabs are frozen or discarded within minutes, sooner on phones. A browser peer is never an always-on peer.
- Private windows keep nothing past the session.
- A browser peer should therefore expect [amnesia](../spec/storage.md), and a group should not need it for a quorum.
- An application that installs to the home screen gets better treatment on every platform checked. It is still not a guarantee.

## Keys

- The core signs synchronously and needs the secret key itself.
- Browser key stores that never reveal the key sign asynchronously and cannot serve as the transport identity, so they are not used.
- The key lives in the store beside the state it signed for, which is what the amnesia rule requires anyway.

## Sources

- File system access handles: MDN and the WHATWG File System standard.
- Storage eviction: MDN's storage quota page and WebKit's storage policy posts.
- Iroh: its documentation on browser support, deployment, and endpoint identity.
- Not verified: whether any browser's flush reaches the disk, Android WebView support for sync handles, and whether a persistence grant exempts an origin from Safari's week limit.
