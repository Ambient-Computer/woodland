# Structure

- `crates/` workspace.

# Tools

- `cargo nextest`
- `cargo clippy`
- `treefmt`

# Developing

- Checks must pass before committing.
- If there's a build/test/run issue, it doesn't matter if it was your session; you're responsible.

# Logging

- Use `tracing` with structured logging. Never interpolate.

# Style

- No FS stutter. If `<name>/` exists, use `<name>/mod.rs` as entrypoint.
- Prefer structure over flat directories.
