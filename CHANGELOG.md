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
- `StoreError::fault` says what went wrong with a store where it could tell: `StoreFault::Busy` when something else has it open, and `StoreFault::Mismatched` when what's under that name isn't a store this version opens.
- `Database::standing` says where a device stands in a group in one answer: `Standing::Member`, `Outside`, `ShutOut`, or `Waiting`.
- `Database::members` lists every peer with a role in a group the device holds, and whoever it has asked for: who to connect to.
- `Database::abandon` deletes a group the device is still a member of, and refuses that group for good. It's for the last member of a group, or a member of one that lost its quorum.
- `Database::settle` resumes until nothing is unfinished, and `Outputs::absorb` adds one step's outputs to another's: the loop every application wrote after each call.
- `Database::delete_row` deletes a row by its key: a read and a guarded delete of what was read.
- `Row::text`, `integer`, `real`, `boolean`, `bytes`, and `blob` give what a row holds in a column by type.
- `VerifyingKey` is re-exported, so an application needn't depend on `ed25519-dalek` to name a peer.
- `Database::is_shut_out` says when a device can no longer read a sealed group it once could, and `Database::relinquish` has it give up what it read and stay to relay the group.

### Changed

- Stores and peers from 0.0.0 are not compatible with this version.
- With the `blobs` feature, `found` and `join` fail until a blob policy is set. There is no default.
- `Store` has two new required methods, `unreturned` and `reclaim`.
- `FileStore::open` keeps at most 64 MiB of the file in memory, down from 1 GiB.
- `compact` on a device that relays a sealed group it can't read now sheds history, by asking a peer for committed state. It used to do nothing.
- `compact` fails with `Error::ShutOut` on a device that was removed from a sealed group, until it calls `forget` or `relinquish`.
- `Error` has three new variants: `NotAMember`, `Abandoned`, and `Bequeathing`. `found` and `join` fail with `Abandoned` for a group the device abandoned.
- The guide no longer says `tick` may be called at any time. Call it on a timer only: calling it for each message received can stop a group committing. `tick` itself is unchanged.

### Fixed

- A device that relays a sealed group no longer grows without bound while it stays up to date.

## [0.0.0] - 2026-11-17

Initial release.

[Unreleased]: https://github.com/Ambient-Computer/woodland/compare/v0.0.0...HEAD
[0.0.0]: https://github.com/Ambient-Computer/woodland/commits/v0.0.0
