# Woodland

A replicated database for devices that are mostly offline.

A few devices share a group. Each writes whenever it likes, online or not, and reads what it has. A quorum of them commits history in checkpoints, and what is committed is final.

- [`docs/spec`](docs/spec/README.md) is the design.
- [`docs/context`](docs/context/README.md) is why.
- `src/lib.rs` says where each part of the design lives.
- [`examples`](examples) show it in use, each over a store in memory with `cargo run --example <name>`:
  - `alone`: one device, a write, and the two levels of read.
  - `devices`: linking a second device, writing while apart, and agreeing.
  - `removal`: removing a lost device, and giving it its role back.
  - `catching_up`: shedding history, joining late, skipping ahead, and restoring.
  - `sealed`: a group only some of its holders can read.
  - `tables`: rows, a change by filter, and an index.
  - `custom_kind`: a kind of model the application defines.
