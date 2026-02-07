# Coop

Coop is a terminal session manager for AI coding agents. It wraps agent CLIs in a PTY (or compatibility layer), monitors their state, and exposes a gRPC API for orchestration.

## Build & Test

```bash
make check    # fmt + clippy + quench + build + test
make ci       # full pre-release (adds audit + deny)
cargo test    # unit tests only
```

## Code Conventions

- License: BUSL-1.1; every source file needs the SPDX header
- Clippy: `unwrap_used`, `expect_used`, `panic` are denied; use `?`, `anyhow::bail!`, or `.ok()`
- Unsafe: forbidden workspace-wide
- Tests: use `-> anyhow::Result<()>` return type instead of unwrap

## Directory Structure

- `crates/cli/` — Main `coop` binary and library crate
  - `src/pty/` — Terminal backend trait and implementations (spawn, attach)
  - `src/driver/` — Agent state detection, nudge/respond encoding
  - `src/screen.rs` — Screen snapshot
  - `src/ring.rs` — Ring buffer
  - `tests/` — Integration tests

## Commits

Use conventional commit format: `type(scope): description`

Types: feat, fix, chore, docs, test, refactor

## Landing the Plane

Before completing work:

1. Run `make check` — all fmt, clippy, quench, build, and test steps must pass
2. Ensure new source files have SPDX license headers
3. Commit with conventional commit format
