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

```toc
crates/cli/               # Single crate (binary + lib)
├── src/
│   ├── main.rs            # CLI, startup
│   ├── lib.rs             # Library root (re-exports modules)
│   ├── error.rs           # ErrorCode enum
│   ├── event.rs           # OutputEvent, StateChangeEvent, InputEvent, HookEvent
│   ├── screen.rs          # Screen, ScreenSnapshot
│   ├── ring.rs            # RingBuffer
│   ├── pty/
│   │   ├── mod.rs         # Backend trait
│   │   ├── nbio.rs        # Non-blocking I/O helpers (PtyFd, AsyncFd)
│   │   └── spawn.rs       # NativePty backend (forkpty + exec)
│   └── driver/
│       ├── mod.rs          # AgentState, Detector, NudgeEncoder traits
│       ├── grace.rs        # IdleGraceTimer
│       └── jsonl_stdout.rs # JsonlParser
└── tests/
    └── pty_backend.rs      # Integration tests for PTY backend
DESIGN.md                   # Full design spec
ROADMAP.md                  # Phased dependency graph
```

## Development

### Quick checks

```sh
make check    # fmt, clippy, quench, build, test
```

### Conventions

- License: BUSL-1.1, Copyright Alfred Jean LLC
- All source files need SPDX license header
- Rust 1.92+: native `async fn` in traits, no `async_trait` macro
- Unit tests in `*_tests.rs` files with `#[cfg(test)] #[path = "..."] mod tests;`

## Commits

Use conventional commit format: `type(scope): description`

Types: feat, fix, chore, docs, test, refactor

## Landing the Plane

Before completing work:

1. Run `make check` — all fmt, clippy, quench, build, and test steps must pass
2. Ensure new source files have SPDX license headers
3. Commit with conventional commit format
