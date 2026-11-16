# Tools

- `cargo nextest`
- `cargo clippy`
- `treefmt`
- `cargo test --target wasm32-unknown-unknown --features browser --test browser` (the browser's store, in headless Firefox)

# Developing

- Checks must pass before committing.
- If there's a build/test/run issue, it doesn't matter if it was your session; you're responsible.

# Logging

- Use `tracing` with structured logging. Never interpolate.

# Style

- No FS stutter. If `<name>/` exists, use `<name>/mod.rs` as entrypoint.
- Prefer structure over flat directories.

# Docs

- `docs/spec` is the design. It leads the code; where they differ, the code is behind.
- `docs/context` is why: who it's for, where it runs, what was rejected.
- Read both before designing. Change the spec before changing the architecture.
