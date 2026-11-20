# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Blobs, behind the `blobs` feature: large bytes that a table cell refers to and each device holds by choice. Import them a piece at a time, read any range, and fetch or release them per device.
- `BlobPolicy` says which blobs a device fetches unasked: all, those up to a size, or none. It can be set per model.
- Blobs in a sealed group are encrypted. Devices that can't read the group still hold and serve them.
- `Outputs::blobs` and `next_blob_message` send blob bytes apart from sync, so a large blob never delays it.
- `Database::unreturned` and `Database::reclaim` report and give back the space a store has freed.
- `FileStore::open_with_cache` sets how much of the file is kept in memory.
- `Database::checkpoint` attempts a checkpoint of one group now, without time passing. Call it after a write or a received message to commit without waiting for a timer.
- `Database::is_shut_out` says when a device can no longer read a sealed group it once could, and `Database::relinquish` has it give up what it read and stay to relay the group.

### Changed

- Stores and peers from 0.0.0 are not compatible with this version.
- With the `blobs` feature, `found` and `join` fail until a blob policy is set. There is no default.
- `Store` has two new required methods, `unreturned` and `reclaim`.
- `FileStore::open` keeps at most 64 MiB of the file in memory, down from 1 GiB.
- `compact` on a device that relays a sealed group it can't read now sheds history, by asking a peer for committed state. It used to do nothing.
- `compact` fails with `Error::ShutOut` on a device that was removed from a sealed group, until it calls `forget` or `relinquish`.

### Fixed

- A device that relays a sealed group no longer grows without bound while it stays up to date.

## [0.0.0] - 2026-11-17

Initial release.

[Unreleased]: https://github.com/Ambient-Computer/woodland/compare/v0.0.0...HEAD
[0.0.0]: https://github.com/Ambient-Computer/woodland/commits/v0.0.0
