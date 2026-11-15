# Woodland

A replicated database for devices that are mostly offline.

A few devices share a group. Each writes whenever it likes, online or not, and reads what it has. A quorum of them commits history in checkpoints, and what is committed is final.

- [`docs/spec`](docs/spec/README.md) is the design.
- [`docs/context`](docs/context/README.md) is why.
- `src/lib.rs` says where each part of the design lives.
