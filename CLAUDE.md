# Coop

Agent terminal sidecar: spawns a child process on a PTY, renders output via
a VTE, classifies agent state from structured data, and serves everything
over HTTP + WebSocket + gRPC.

## Architecture Overview

Coop is a standalone Rust binary with four progressive layers:

1. **PTY + VTE** (always on) — Spawn child, read output, render screen, ring buffer
2. **Detection** (opt-in via `--agent-type`) — Classify agent state from structured sources
3. **Nudge** (when driver active) — Mechanically deliver a message to an idle agent
4. **Respond** (when driver active) — Mechanically answer a prompt the agent asked

Coop provides capability. Consumers provide intent. The driver never writes
to the PTY on its own — all input comes from explicit API calls.

## Directory Structure

```toc
crates/cli/               # Single crate (binary)
├── src/
│   ├── main.rs            # CLI, startup
│   ├── error.rs           # ErrorCode enum
│   ├── event.rs           # OutputEvent, StateChangeEvent, InputEvent, HookEvent
│   ├── screen.rs          # Screen, ScreenSnapshot
│   ├── ring.rs            # RingBuffer
│   ├── pty/
│   │   └── mod.rs         # Backend trait
│   └── driver/
│       ├── mod.rs          # AgentState, Detector, NudgeEncoder traits
│       ├── grace.rs        # IdleGraceTimer
│       └── jsonl_stdout.rs # JsonlParser
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

## Landing the Plane

Before committing changes:

- [ ] Run `make check` for local verification
  - `cargo fmt --all`
  - `cargo clippy --all -- -D warnings`
  - `quench check --fix --no-cloc`
  - `cargo build --all`
  - `cargo test --all`

## Commits

Use conventional commit format: `type: description`
Types: feat, fix, chore, docs, test, refactor
