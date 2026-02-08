# 14a Session Switch

Consumer-driven account switching via env override. No registry, no
auto-rotation — the caller says what to switch to and coop executes
the restart.

## Motivation

Coop owns the PTY, the child lifecycle, and the session ID. Swapping
credentials means killing the child and re-spawning with different env
vars. Only the PTY owner can do this cleanly. The consumer (oddjobs,
Gas Town) knows *when* and *to what* — coop knows *how*.

## Endpoint

```
POST /api/v1/session/switch
```

Request:

```json
{
  "env": { "CLAUDE_CONFIG_DIR": "/path/to/other/profile" },
  "force": false,
  "timeout_secs": 30
}
```

- `env` — key-value pairs merged into the child's environment on
  respawn. Overrides existing vars, does not clear unmentioned ones.
- `force` — if `false` (default), wait for `WaitingForInput` before
  killing. If `true`, kill immediately.
- `timeout_secs` — max seconds to wait for idle when `force: false`.
  Returns an error if the agent doesn't become idle in time.

Response (success):

```json
{
  "switched": true,
  "session_id": "abc123",
  "duration_ms": 2340
}
```

Response (agent busy, not forced):

```json
{
  "switched": false,
  "reason": "agent_busy",
  "state": "working"
}
```

The endpoint should also be exposed on WebSocket (`{ "type":
"switch_session", "env": {...} }`) and gRPC (`SwitchSession` RPC)
following existing transport patterns.

## Agent State

Add `Switching` to `AgentState`:

```rust
pub enum AgentState {
    // ... existing ...
    Switching,
}
```

Emitted when the switch begins, transitions to `Starting` when the
new child is spawned. Consumers see a normal `Starting` → `Working`
/ `WaitingForInput` progression after.

## Switch Flow

```diagram
POST /session/switch { env, force, timeout_secs }
    │
    ├─ force: true ──────────────────────┐
    │                                     │
    ├─ agent is WaitingForInput? ────────┤
    │   └─ No → wait up to timeout_secs  │
    │       ├─ idle within timeout ──────┤
    │       └─ timeout → return error    │
    │                                     │
    ▼                                     │
Set AgentState::Switching ◄──────────────┘
    │
    ▼
Grab session ID (from Claude driver's tracked session log path)
    │
    ▼
SIGHUP to child → wait grace_period → SIGKILL if needed
    │
    ▼
Backend exits, session loop detects EOF
    │
    ▼
Merge env overrides into child environment
    │
    ▼
Build resume command: original args + --resume <session-id>
    │
    ▼
Spawn new child via Backend, re-initialize detectors
    │
    ▼
Set AgentState::Starting
    │
    ▼
Resume normal session loop
```

## Session Loop Changes

Today `Session::run` breaks on backend EOF and returns the exit
status. For switching, the session loop needs a restart path:

1. Add a `switch_rx: mpsc::Receiver<SwitchRequest>` to `Session`.
   The HTTP handler sends switch requests through this channel.

2. Add a `pending_switch: Option<SwitchRequest>` field. When a switch
   is requested and the agent is idle (or force), set the pending
   switch, then SIGHUP the child.

3. On backend EOF, check `pending_switch`:
   - `Some(req)` → execute the switch (new backend, new detectors),
     continue the loop.
   - `None` → normal exit (break).

This keeps the select loop structure intact. The "restart" is just
building a new backend and detector set within the existing loop
iteration.

### What restarts

- Backend (new PTY, new child process, new env)
- Detectors (new session log path, new hook pipe)
- Screen + ring buffer get **cleared** (new session, clean slate)

### What persists

- AppState (same Arc, same channels, same HTTP server)
- Transport connections (HTTP/WS/gRPC stay up)
- Consumer input channel (same sender)

## Session ID Extraction

The Claude driver's `LogDetector` already knows the session log path
(e.g. `~/.claude/sessions/<id>/session.jsonl`). The session ID is the
parent directory name. No new parsing needed — just expose it.

Add to the Claude driver:

```rust
impl ClaudeDriver {
    /// Extract session ID from the watched log path.
    pub fn session_id(&self) -> Option<String> {
        self.session_log_path.as_ref()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
    }
}
```

For non-Claude agents or when no session log is configured, `--resume`
is omitted from the respawn command and the child starts fresh.

## Edge Cases

1. **Switch during switch** — reject with 409. Only one switch at a
   time.
2. **Switch while exited** — reject. Nothing to switch.
3. **Resume failure** — if the new child exits immediately after
   spawn, the session loop sees normal EOF and exits. The consumer
   can retry or investigate.
4. **Concurrent input during switch** — input channel stays open but
   the backend input sender is dropped during the gap. Writes return
   errors until the new backend is wired up.

## Files

- Modify `src/driver/mod.rs` — add `Switching` to `AgentState`
- Modify `src/session.rs` — add `switch_rx`, pending switch, restart logic
- Modify `src/transport/http.rs` — add `POST /session/switch` handler
- Modify `src/transport/ws.rs` — add `switch_session` message type
- Modify `src/transport/grpc.rs` — add `SwitchSession` RPC
- Modify `src/transport/state.rs` — add switch channel sender to AppState
- Modify `src/driver/claude/mod.rs` — expose `session_id()`
- Modify `src/main.rs` — wire switch channel into Session and AppState
- Create `src/session_switch_tests.rs` or extend `src/session_tests.rs`

## Size Estimate

~400 lines impl + ~250 lines tests
