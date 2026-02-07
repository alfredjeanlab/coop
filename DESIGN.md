# Coop Design

Agent terminal sidecar: spawns a child process on a PTY, renders output via
a VTE, classifies agent state from structured data, and serves everything
over HTTP + WebSocket + gRPC.


## 1. Overview

### What coop is

Coop is a standalone Rust binary that:

1. Spawns a child process (any AI coding agent) on a pseudo-terminal
2. Parses terminal output through a VTE (the `avt` crate, extracted from capsh)
3. Classifies agent state via built-in drivers using structured data sources
4. Serves terminal state, agent state, nudge, and prompt response over HTTP, WebSocket, and gRPC

### Four progressive layers

| Layer | Always on? | What it does |
|-------|-----------|--------------|
| **PTY + VTE** | Yes | Spawn child, read output, render screen, ring buffer |
| **Detection** | Opt-in (`--agent-type`) | Classify agent state from structured sources, emit events |
| **Nudge** | When driver active | Mechanically deliver a message to an idle agent |
| **Respond** | When driver active | Mechanically answer a prompt the agent asked |

Coop provides capability. Consumers provide intent.

The driver never writes to the PTY on its own. All input comes from
consumers via the API. A permission prompt is a state change event.
Whether to accept it, escalate it, or kill the agent is the consumer's
decision.

### What it replaces

| Before | After |
|--------|-------|
| tmux + `tmux capture-pane` | `GET /api/v1/screen` |
| tmux + `tmux send-keys` | `POST /api/v1/input` |
| screen + `screen -x` attach | WebSocket `/ws` |
| SSH + tmux for remote agents | HTTP (direct or via K8s) |
| kubectl exec + screen | Coop sidecar in pod |
| Screen parsing in Gas Town | `GET /api/v1/agent/state` |
| Session log parsing in oddjobs | Coop state change events |
| Nudge protocol in consumers | `POST /api/v1/agent/nudge` |
| Prompt handling in consumers | `POST /api/v1/agent/respond` |

### Why it exists

- **tmux is not an API.** `capture-pane` returns raw text with no structure.
- **tmux is local.** Remote agents need SSH + tmux or kubectl exec + screen.
- **tmux serializes writers.** Concurrent nudges corrupt each other.
- **No VTE.** Rendered text requires a separate terminal emulator.
- **Screen parsing is duplicated.** Gas Town and oddjobs both implement
  Claude-specific state detection. When the TUI changes, both break.
- **Screen parsing is unreliable.** A `>` could be a prompt, a quote, or
  output. The gap between tool calls looks identical to "agent is stuck."


## 2. Architecture

```
                         Consumers
                ┌─────────────────────────┐
                │  Gas Town (Go)          │
                │  Oddjobs (Rust)         │
                │                         │
                │  "nudge agent with X"   │
                │  "accept permission"    │
                │  "subscribe to events"  │
                └────────────┬────────────┘
                             │ HTTP / WebSocket / gRPC
                ┌────────────▼────────────┐
                │         coop            │
                │                         │
                │  ┌───────────────────┐  │
                │  │  Transport Layer  │  │
                │  │  HTTP + WS + gRPC │  │
                │  └────────┬──────────┘  │
                │           │             │
                │  ┌────────▼──────────┐  │
                │  │  Driver Layer     │  │
                │  │  detection tiers  │  │
                │  │  nudge + respond  │  │
                │  └────────┬──────────┘  │
                │           │             │
                │  ┌────────▼──────────┐  │
                │  │  Terminal Backend │  │
                │  │  PTY + VTE (avt)  │  │
                │  └────────┬──────────┘  │
                └───────────┼─────────────┘
                            │ PTY master fd
                ┌───────────▼─────────────┐
                │     Child Process       │
                │  (claude, codex, etc.)  │
                └─────────────────────────┘
```

### Data Flow

```
  Child stdout ──► PTY master fd ──► read loop ──► ring buffer
                                         │
                                         ▼
                                    avt VTE feed
                                         │
                                    ┌────▼────┐
                                    │  Screen │
                                    └────┬────┘
                                         │
                    ┌────────────────────┼────────────────────┐
                    ▼                    ▼                    ▼
             GET /screen        detection tiers          WS /ws
                              (log, hooks, stdout,      (stream)
                               process, screen)
                                    │
                                    ▼
                              state events
                              ──► consumers

  POST /input ───────────────────────► PTY write ──► Child stdin
  POST /agent/nudge ──► encode ──────► PTY write ──► Child stdin
  POST /agent/respond ──► encode ────► PTY write ──► Child stdin
```

### Session Loop

```rust
pub struct Session {
    backend: Box<dyn Backend>,
    screen: Screen,
    detector: CompositeDetector,
    agent_state: AgentState,
    output_tx: broadcast::Sender<OutputEvent>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    input_tx: mpsc::Sender<InputEvent>,
    input_rx: mpsc::Receiver<InputEvent>,
    detector_rx: mpsc::Receiver<AgentState>,
}

impl Session {
    pub async fn run(mut self) -> anyhow::Result<i32> {
        let mut buf = [0u8; 8192];
        let mut screen_debounce = tokio::time::interval(Duration::from_millis(50));

        loop {
            tokio::select! {
                // Read from PTY
                result = self.backend.read(&mut buf) => {
                    match result? {
                        0 => break, // EOF
                        n => {
                            let data = Bytes::copy_from_slice(&buf[..n]);
                            let _ = self.output_tx.send(OutputEvent::Raw(data));
                            self.screen.feed(&buf[..n]);
                        }
                    }
                }

                // Process input from consumers
                Some(input) = self.input_rx.recv() => {
                    match input {
                        InputEvent::Write(data) => self.backend.write(&data).await?,
                        InputEvent::Resize { cols, rows } => {
                            self.backend.resize(cols, rows)?;
                            self.screen.resize(cols, rows);
                        }
                        InputEvent::Signal(sig) => self.backend.signal(sig)?,
                    }
                }

                // State changes from detector
                Some(new_state) = self.detector_rx.recv() => {
                    if new_state != self.agent_state {
                        let event = StateChangeEvent {
                            prev: self.agent_state.clone(),
                            next: new_state.clone(),
                            seq: self.screen.seq(),
                        };
                        let _ = self.state_tx.send(event);
                        self.agent_state = new_state;
                    }
                }

                // Screen update broadcast
                _ = screen_debounce.tick() => {
                    if self.screen.changed() {
                        let _ = self.output_tx.send(
                            OutputEvent::ScreenUpdate { seq: self.screen.seq() },
                        );
                    }
                }
            }
        }

        let code = self.backend.wait().await?;
        Ok(code)
    }
}
```

The detector runs as its own async task (or set of tasks), pushing state
changes into `detector_rx`. This decouples detection from the main I/O
loop — file watchers and grace timers don't block PTY reads.

### Task Model

- **Main task**: `Session::run` select loop
- **Detector tasks**: one per active detection tier (file watcher, hook reader, etc.)
- **Grace timer task**: verifies idle after 60s delay
- **HTTP server task**: axum on TCP or Unix socket
- **gRPC server task**: tonic (optional)
- **Per-WebSocket task**: subscribes to output and/or state broadcasts
- **Signal task**: SIGTERM/SIGINT graceful shutdown
- **Health probe task**: separate TCP listener (optional)


## 3. Terminal Backend

### Backend Trait

```rust
trait Backend: Send + 'static {
    async fn run(
        &mut self,
        output_tx: mpsc::Sender<Bytes>,
        mut input_rx: mpsc::Receiver<Bytes>,
    ) -> Result<ExitStatus>;

    fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    fn child_pid(&self) -> Option<u32>;
}

struct ExitStatus {
    code: Option<i32>,
    signal: Option<i32>,
}
```

### Native PTY Backend (Primary)

Spawns a child on a new PTY via `forkpty()`. Child exec's
`/bin/sh -c <command>`. Environment overrides:

```
TERM=xterm-256color
COOP=1
COOP_SOCKET=<path>
```

**Read loop.** Master fd in non-blocking mode, wrapped in tokio `AsyncFd`.
Reads 8 KiB chunks → ring buffer → avt VTE → broadcast.

**Ring buffer.** 1 MiB circular buffer for raw PTY output. Supports
read-from-offset for WebSocket replay.

```rust
struct RingBuffer {
    buf: Vec<u8>,
    capacity: usize,
    write_pos: usize,
    total_written: u64,
}
```

**Resize.** `TIOCSWINSZ` ioctl + avt VTE resize.

**Screen.** Wraps `avt::Vt`:

```rust
impl Screen {
    fn feed(&mut self, data: &[u8]);
    fn snapshot(&self) -> ScreenSnapshot;
    fn is_alt_screen(&self) -> bool;
    fn changed(&self) -> bool;
    fn seq(&self) -> u64;
    fn resize(&mut self, cols: u16, rows: u16);
}
```

### Tmux Compat Backend

Attaches to existing tmux session. Polls `capture-pane -p -e` at 1s.
Input via `send-keys -l`. Resize via `resize-pane`. For migration only.

### Screen Compat Backend

Attaches to existing GNU Screen session. Polls `hardcopy` at 1s.
Input via `-X stuff`. No ANSI color. For migration only.

### Backend Comparison

| Feature | Native PTY | Tmux | Screen |
|---------|-----------|------|--------|
| Output source | PTY master fd | `capture-pane` | `hardcopy` |
| Latency | Real-time | ~1s poll | ~1s poll |
| ANSI | Yes | Yes | No |
| Ring buffer | Yes | No | No |
| Alt screen | Tracked | Unreliable | N/A |
| Detection accuracy | Full | Degraded | Degraded |

### Graceful Shutdown

1. Stop accepting new connections
2. Send `exit` to WebSocket clients
3. SIGHUP to child → wait 10s → SIGKILL
4. Close connections, exit


## 4. Transport

### Listener Modes

| Mode | Flag | Use case |
|------|------|----------|
| TCP | `--port 8080` | K8s sidecar, remote |
| Unix socket | `--socket PATH` | Local dev |

Both can be active simultaneously.

### Authentication

Token-based, disabled by default.

```
--auth-token <TOKEN>
HTTP/gRPC: Authorization: Bearer <TOKEN>
WebSocket: /ws?token=<TOKEN> or { "type": "auth", "token": "<TOKEN>" }
```

### Concurrency

**Single writer.** HTTP POST acquires/releases atomically. WebSocket clients
acquire via `{ "type": "lock" }`. Auto-releases after 30s. 409 if held.

**Reader fan-out.** Multiple concurrent readers, no locks.

### HTTP Endpoints

#### Terminal endpoints (always available)

**GET /api/v1/health**

```json
{
  "status": "running",
  "pid": 12345,
  "uptime_secs": 3600,
  "agent_type": "claude",
  "terminal": { "cols": 200, "rows": 50 },
  "ws_clients": 2
}
```

**GET /api/v1/screen** `?format=text|ansi` `?cursor=bool`

```json
{
  "lines": ["$ claude", "..."],
  "rows": 50,
  "cols": 200,
  "cursor": { "row": 5, "col": 12 },
  "alt_screen": false,
  "sequence": 42
}
```

**GET /api/v1/screen/text**

Plain text body. One line per row. For `curl`.

**GET /api/v1/output** `?offset=u64` `?limit=usize`

```json
{
  "data": "<base64>",
  "offset": 1024,
  "next_offset": 2048,
  "total_written": 2048
}
```

**POST /api/v1/input**

```json
{ "text": "hello", "enter": true }
→ { "bytes_written": 6 }
```

**POST /api/v1/input/keys**

```json
{ "keys": ["Escape", "Enter", "Ctrl-C"] }
→ { "bytes_written": 3 }
```

**POST /api/v1/resize**

```json
{ "cols": 200, "rows": 50 }
```

**POST /api/v1/signal**

```json
{ "signal": "SIGINT" }
```

**GET /api/v1/status**

```json
{
  "state": "running",
  "pid": 12345,
  "exit_code": null,
  "screen_seq": 4217,
  "bytes_read": 1048576,
  "bytes_written": 2048,
  "ws_clients": 2
}
```

#### Agent endpoints (require `--agent-type`)

**GET /api/v1/agent/state**

Returns classified agent state with prompt context when applicable.

```json
{
  "agent_type": "claude",
  "state": "waiting_for_input",
  "since_seq": 4210,
  "screen_seq": 4217,
  "detection_tier": "session_log",
  "idle_grace_remaining_secs": null,
  "prompt": null
}
```

When the agent is at a prompt:

```json
{
  "agent_type": "claude",
  "state": "permission_prompt",
  "since_seq": 4215,
  "screen_seq": 4217,
  "detection_tier": "session_log",
  "prompt": {
    "type": "permission",
    "tool": "Bash",
    "input_preview": "npm install express",
    "screen_lines": [
      "Claude wants to use Bash: npm install express",
      "[y] Yes  [n] No"
    ]
  }
}
```

```json
{
  "state": "ask_user",
  "prompt": {
    "type": "question",
    "question": "Which database should we use?",
    "options": ["PostgreSQL (Recommended)", "SQLite", "MySQL"],
    "screen_lines": ["..."]
  }
}
```

```json
{
  "state": "plan_prompt",
  "prompt": {
    "type": "plan",
    "summary": "Implementation plan for auth system",
    "screen_lines": ["..."]
  }
}
```

Agent states:

| State | Meaning |
|-------|---------|
| `starting` | Launched, no prompt yet |
| `working` | Producing output (thinking, tool use) |
| `waiting_for_input` | Idle at prompt |
| `permission_prompt` | Asking for tool permission |
| `plan_prompt` | Presenting a plan for approval |
| `ask_user` | Asking a question with options |
| `error` | Showing an error (rate limit, auth, crash) |
| `alt_screen` | In alternate screen (editor, pager) |
| `exited` | Child process exited |
| `unknown` | Driver cannot classify |

When `--agent-type unknown`, state is always `unknown` (except `exited`).

**POST /api/v1/agent/nudge**

Deliver a message to an idle agent. Coop encodes the agent-specific input
protocol (write text, send Enter).

```json
{ "message": "Fix the login bug in auth.go" }

// Success
→ { "delivered": true, "state_before": "waiting_for_input" }

// Agent not ready
→ { "delivered": false, "reason": "agent_busy", "state": "working" }
```

Does not block until the agent finishes. Delivers the message and returns.
Consumers watch state events to track progress.

**POST /api/v1/agent/respond**

Answer a prompt the agent asked. Coop encodes the correct keystrokes.

```json
// Accept a permission prompt
{ "accept": true }
→ { "delivered": true, "prompt_type": "permission" }

// Deny a permission prompt
{ "accept": false }
→ { "delivered": true, "prompt_type": "permission" }

// Select an option (AskUser)
{ "option": 2 }
→ { "delivered": true, "prompt_type": "question" }

// Freeform text response (AskUser with "Other")
{ "text": "Use Redis instead" }
→ { "delivered": true, "prompt_type": "question" }

// Approve a plan
{ "accept": true }
→ { "delivered": true, "prompt_type": "plan" }

// Reject a plan with feedback
{ "accept": false, "text": "Don't modify the database schema" }
→ { "delivered": true, "prompt_type": "plan" }

// No prompt active
→ { "delivered": false, "reason": "no_prompt", "state": "working" }
```

### WebSocket Protocol

`GET /ws` upgrades to WebSocket. Tagged JSON messages.

Subscription modes: `?mode=raw|screen|state|all` (default: `all`).

#### Server to client

```json
{ "type": "output", "data": "base64...", "offset": 1024 }

{ "type": "screen", "lines": [...], "cols": 200, "rows": 50,
  "alt_screen": false, "cursor": { "row": 5, "col": 12 }, "seq": 42 }

{ "type": "state_change", "prev": "working", "next": "permission_prompt",
  "seq": 4217, "prompt": { "type": "permission", "tool": "Bash",
  "input_preview": "npm install" } }

{ "type": "exit", "code": 0, "signal": null }

{ "type": "error", "message": "...", "code": "..." }

{ "type": "resize", "cols": 200, "rows": 50 }

{ "type": "pong" }
```

#### Client to server

```json
{ "type": "input", "text": "hello\r" }
{ "type": "input_raw", "data": "base64..." }
{ "type": "keys", "keys": ["Escape"] }
{ "type": "resize", "cols": 200, "rows": 50 }
{ "type": "screen_request" }
{ "type": "state_request" }
{ "type": "nudge", "message": "Fix the bug" }
{ "type": "respond", "accept": true }
{ "type": "respond", "option": 2 }
{ "type": "respond", "text": "Use Redis" }
{ "type": "replay", "offset": 0 }
{ "type": "lock", "action": "acquire" }
{ "type": "lock", "action": "release" }
{ "type": "auth", "token": "..." }
{ "type": "ping" }
```

### gRPC

Optional, enabled via `--grpc-port`.

```protobuf
syntax = "proto3";
package coop.v1;

service Coop {
  // Terminal
  rpc GetHealth(GetHealthRequest) returns (GetHealthResponse);
  rpc GetScreen(GetScreenRequest) returns (GetScreenResponse);
  rpc GetStatus(GetStatusRequest) returns (GetStatusResponse);
  rpc SendInput(SendInputRequest) returns (SendInputResponse);
  rpc SendKeys(SendKeysRequest) returns (SendKeysResponse);
  rpc Resize(ResizeRequest) returns (ResizeResponse);
  rpc SendSignal(SendSignalRequest) returns (SendSignalResponse);
  rpc StreamOutput(StreamOutputRequest) returns (stream OutputChunk);
  rpc StreamScreen(StreamScreenRequest) returns (stream ScreenSnapshot);

  // Agent
  rpc GetAgentState(GetAgentStateRequest) returns (GetAgentStateResponse);
  rpc Nudge(NudgeRequest) returns (NudgeResponse);
  rpc Respond(RespondRequest) returns (RespondResponse);
  rpc StreamState(StreamStateRequest) returns (stream AgentStateEvent);
}

// --- Terminal messages ---

message GetHealthRequest {}
message GetHealthResponse {
  string status = 1;
  optional int32 pid = 2;
  int64 uptime_secs = 3;
  string agent_type = 4;
  int32 ws_clients = 5;
}

message GetScreenRequest {
  enum Format { TEXT = 0; ANSI = 1; }
  Format format = 1;
  bool include_cursor = 2;
}

message GetScreenResponse {
  repeated string lines = 1;
  int32 cols = 2;
  int32 rows = 3;
  bool alt_screen = 4;
  CursorPosition cursor = 5;
  uint64 sequence = 6;
}

message CursorPosition {
  int32 row = 1;
  int32 col = 2;
}

message GetStatusRequest {}
message GetStatusResponse {
  string state = 1;
  optional int32 pid = 2;
  int64 uptime_secs = 3;
  optional int32 exit_code = 4;
  uint64 screen_seq = 5;
  uint64 bytes_read = 6;
  uint64 bytes_written = 7;
  int32 ws_clients = 8;
}

message SendInputRequest {
  string text = 1;
  bool enter = 2;
}
message SendInputResponse {
  int32 bytes_written = 1;
}

message SendKeysRequest {
  repeated string keys = 1;
}
message SendKeysResponse {
  int32 bytes_written = 1;
}

message ResizeRequest {
  int32 cols = 1;
  int32 rows = 2;
}
message ResizeResponse {
  int32 cols = 1;
  int32 rows = 2;
}

message SendSignalRequest {
  string signal = 1;
}
message SendSignalResponse {
  bool delivered = 1;
}

message StreamOutputRequest {
  uint64 from_offset = 1;
}
message OutputChunk {
  bytes data = 1;
  uint64 offset = 2;
}

message StreamScreenRequest {}
message ScreenSnapshot {
  repeated string lines = 1;
  int32 cols = 2;
  int32 rows = 3;
  bool alt_screen = 4;
  CursorPosition cursor = 5;
  uint64 sequence = 6;
}

// --- Agent messages ---

message PromptContext {
  string type = 1;                      // "permission", "question", "plan"
  optional string tool = 2;             // for permission prompts
  optional string input_preview = 3;    // for permission prompts
  optional string question = 4;         // for ask_user
  repeated string options = 5;          // for ask_user
  optional string summary = 6;          // for plan prompts
  repeated string screen_lines = 7;     // raw screen context
}

message GetAgentStateRequest {}
message GetAgentStateResponse {
  string agent_type = 1;
  string state = 2;
  uint64 since_seq = 3;
  uint64 screen_seq = 4;
  string detection_tier = 5;
  optional PromptContext prompt = 6;
  optional float idle_grace_remaining_secs = 7;
}

message NudgeRequest {
  string message = 1;
}
message NudgeResponse {
  bool delivered = 1;
  string state_before = 2;
  optional string reason = 3;
}

message RespondRequest {
  optional bool accept = 1;            // for permission/plan
  optional int32 option = 2;           // for ask_user (1-indexed)
  optional string text = 3;            // freeform text
}
message RespondResponse {
  bool delivered = 1;
  string prompt_type = 2;
  optional string reason = 3;
}

message StreamStateRequest {}
message AgentStateEvent {
  string prev = 1;
  string next = 2;
  uint64 seq = 3;
  optional PromptContext prompt = 4;
}
```

### Error Handling

| Condition | HTTP | WebSocket | gRPC |
|-----------|------|-----------|------|
| Child not started | 503 `NOT_READY` | Close 4503 | UNAVAILABLE |
| Child exited | 410 `EXITED` | `exit` msg | NOT_FOUND |
| Writer busy | 409 `WRITER_BUSY` | `error` msg | RESOURCE_EXHAUSTED |
| Auth failed | 401 `UNAUTHORIZED` | Close 4401 | UNAUTHENTICATED |
| Bad request | 400 `BAD_REQUEST` | `error` msg | INVALID_ARGUMENT |
| No driver | 404 `NO_DRIVER` | `error` msg | UNIMPLEMENTED |
| Agent busy (nudge) | 409 `AGENT_BUSY` | `error` msg | FAILED_PRECONDITION |
| No prompt (respond) | 409 `NO_PROMPT` | `error` msg | FAILED_PRECONDITION |
| Backend error | 500 `INTERNAL` | `error` msg | INTERNAL |


## 5. Driver Layer

### Design Principles

1. **Never writes to PTY.** The driver detects state and reports it.
   All input comes from consumers via `/input`, `/nudge`, or `/respond`.
2. **Structured detection.** Uses agent log files, hooks, and structured
   stdout — not screen regex. Screen parsing is a last-resort fallback.
3. **Grace timer.** Prevents false idle triggers between rapid tool calls.
4. **Consumers decide.** Permission prompts, plan approvals, and questions
   are events. Coop reports them. The consumer chooses the response.

### Detection Tiers

Each tier is more reliable than the one below. Coop uses the highest
available tier for the current agent.

```
Tier 1: Hook events                (push-based, real-time)
Tier 2: Session log watching       (file-based, structured JSONL)
Tier 3: Structured stdout parsing  (JSONL from PTY output stream)
Tier 4: Process + PTY activity     (universal, no agent knowledge)
Tier 5: Screen parsing             (last resort, regex on rendered text)
```

**Default tiers by agent type:**

| Agent | Tier 1 (hooks) | Tier 2 (log) | Tier 3 (stdout) | Tier 4 (process) | Tier 5 (screen) |
|-------|---------------|-------------|----------------|-----------------|----------------|
| `claude` | PostToolUse, Stop | `~/.claude/sessions/` | `--print --output-format stream-json` | Yes | No |
| `codex` | — | — | `--json` JSONL | Yes | No |
| `gemini` | AfterTool, SessionEnd | `~/.gemini/tmp/<hash>/chats/` | `stream-json` JSONL | Yes | No |
| `unknown` | — | — | — | Yes | Optional via `--agent-config` |

Claude supports all three structured tiers. Tier 1 (hooks) provides
real-time push events. Tier 2 (session log) is the reliable fallback
when running interactively since the log file exists regardless of
output flags. Tier 3 (stdout JSONL) is useful when running Claude in
non-interactive mode with `--print --output-format stream-json`.
Note that `--output-format stream-json` requires `--print`.

### Shared Infrastructure

#### Detector Trait and Composite

Each tier implements `Detector`. The `CompositeDetector` runs all active
tiers and takes the highest-confidence signal.

```rust
#[async_trait]
trait Detector: Send + 'static {
    async fn run(self, state_tx: mpsc::Sender<AgentState>, shutdown: CancellationToken);
    fn tier(&self) -> u8;
}

struct CompositeDetector {
    tiers: Vec<Box<dyn Detector>>,
    grace_timer: IdleGraceTimer,
    state_tx: mpsc::Sender<AgentState>,
}
```

#### Idle Grace Timer

Adopted from oddjobs. Prevents false idle between rapid tool calls.

```rust
struct IdleGraceTimer {
    duration: Duration,               // 60s default
    pending: Option<GraceState>,
}

struct GraceState {
    triggered_at: Instant,
    log_size_at_trigger: u64,
    timer: tokio::time::Sleep,
}
```

When any tier reports `WaitingForInput`:

1. Record session log byte size (or ring buffer offset)
2. Start 60s timer
3. Timer fires → verify log hasn't grown AND state is still idle
4. Both pass → emit `WaitingForInput` to consumers
5. Either fails → cancel, agent was active

#### Hook Pipe (Tier 1 infrastructure)

Coop creates a named pipe and reads structured events from it. Agent
hooks write JSON to the pipe. Used by `driver/claude/` and
`driver/gemini/`.

```rust
struct HookReceiver {
    pipe: AsyncFd<File>,
}

impl HookReceiver {
    async fn next_event(&self) -> Option<HookEvent> {
        // Read JSON line from pipe
    }
}

enum HookEvent {
    ToolComplete { tool: String },
    AgentStop,
    SessionEnd,
}
```

Hook events are the highest-confidence signal — when a `Stop` or
`AgentStop` hook fires, the agent is definitively idle.

#### Session Log Watcher (Tier 2 infrastructure)

Watches a session file for changes. Reads new lines from a tracked byte
offset. Used by `driver/claude/` and `driver/gemini/`.

```rust
struct LogWatcher {
    path: PathBuf,
    offset: u64,
    watcher: notify::RecommendedWatcher,
    poll_fallback: Duration,     // 5s
}

impl LogWatcher {
    fn check(&mut self) -> Vec<String> {
        // Read new lines since last offset
    }
}
```

#### JSONL Stdout Parser (Tier 3 infrastructure)

Scans raw PTY bytes for complete JSON lines. Used by `driver/codex/`
and `driver/gemini/`.

```rust
struct JsonlParser {
    line_buf: Vec<u8>,
}

impl JsonlParser {
    fn feed(&mut self, data: &[u8]) -> Vec<Value> {
        let mut entries = Vec::new();
        for &byte in data {
            if byte == b'\n' {
                if let Ok(json) = serde_json::from_slice::<Value>(&self.line_buf) {
                    entries.push(json);
                }
                self.line_buf.clear();
            } else {
                self.line_buf.push(byte);
            }
        }
        entries
    }
}
```

#### Process Monitor (Tier 4 — all agents)

No agent-specific knowledge. Detects:
- Is the child alive? (`kill(pid, 0)`)
- Is the PTY active? (ring buffer grown in last 30s?)
- Has the child exited? (EOF on PTY, waitpid)

Cannot distinguish "working but quiet" from "idle." Coarse backstop.

#### Screen Parser (Tier 5 — unknown agents only)

Regex on rendered VTE output. Configured via `--agent-config`.

```rust
struct ScreenParser {
    prompt_pattern: Regex,
    working_patterns: Vec<Regex>,
    error_patterns: Vec<Regex>,
}
```

Not used for Claude, Codex, or Gemini.

#### Nudge and Respond Traits

```rust
struct NudgeStep {
    bytes: Vec<u8>,
    delay_after: Option<Duration>,
}

trait NudgeEncoder {
    fn encode(&self, message: &str) -> Vec<NudgeStep>;
}

trait RespondEncoder {
    fn encode_permission(&self, accept: bool) -> Vec<NudgeStep>;
    fn encode_plan(&self, accept: bool, feedback: Option<&str>) -> Vec<NudgeStep>;
    fn encode_question(&self, option: Option<u32>, text: Option<&str>) -> Vec<NudgeStep>;
}
```

### Claude Driver (`driver/claude/`)

Uses Tier 1 (hooks) + Tier 2 (session log) + Tier 3 (stdout JSONL, when
`--print --output-format stream-json`) + Tier 4 (process).

#### Log Discovery

1. Check `CLAUDE_CONFIG_DIR` env var
2. Default: `~/.claude/sessions/`
3. Watch sessions directory for new `.jsonl` file after spawn
4. Or: pass `--session-id <uuid>` to claude at spawn for a known log path

#### State Parsing (`driver/claude/state.rs`)

Parses Claude's session log JSONL:

```rust
fn parse_claude_state(json: &Value) -> Option<AgentState> {
    // Error check
    if let Some(error) = json.get("error") {
        return Some(AgentState::Error {
            detail: error.as_str().unwrap_or("unknown").to_string(),
        });
    }

    // Only assistant messages carry state
    if json.get("type").and_then(|v| v.as_str()) != Some("assistant") {
        return Some(AgentState::Working);
    }

    let content = json.get("message")?.get("content")?.as_array()?;

    for block in content {
        let block_type = block.get("type").and_then(|v| v.as_str());
        match block_type {
            Some("tool_use") => {
                let tool = block.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                return match tool {
                    "AskUserQuestion" => Some(AgentState::AskUser {
                        prompt: extract_ask_user_context(block),
                    }),
                    _ => Some(AgentState::Working),
                };
            }
            Some("thinking") => return Some(AgentState::Working),
            _ => {}
        }
    }

    Some(AgentState::WaitingForInput)
}
```

#### Hook Config (`driver/claude/hooks.rs`)

Coop creates a named pipe and registers hooks before spawn:

```json
{
  "hooks": {
    "PostToolUse": [{
      "type": "command",
      "command": "echo '{\"event\":\"post_tool_use\",\"tool\":\"$TOOL_NAME\"}' > $COOP_HOOK_PIPE"
    }],
    "Stop": [{
      "type": "command",
      "command": "echo '{\"event\":\"stop\"}' > $COOP_HOOK_PIPE"
    }]
  }
}
```

Hook events complement log watching — when `Stop` fires, the agent is
definitively idle.

#### Prompt Context (`driver/claude/prompt.rs`)

Extracts structured context from the session log:

```rust
// Permission prompt — from pending tool_use block
fn extract_permission_context(json: &Value) -> PromptContext {
    let tool_use = find_pending_tool_use(json);
    PromptContext {
        prompt_type: "permission",
        tool: tool_use.get("name").as_str(),
        input_preview: summarize_tool_input(tool_use.get("input")),
        screen_lines: None,
    }
}

// AskUser — from AskUserQuestion tool_use input
fn extract_ask_user_context(block: &Value) -> PromptContext {
    let input = block.get("input").unwrap();
    PromptContext {
        prompt_type: "question",
        question: input.get("question").as_str(),
        options: input.get("options").as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect()),
        screen_lines: None,
    }
}

// Plan — session log records plan mode entry; screen lines provide content
fn extract_plan_context(screen: &ScreenSnapshot) -> PromptContext {
    PromptContext {
        prompt_type: "plan",
        summary: None, // extracted from screen if possible
        screen_lines: Some(screen.lines.clone()),
    }
}
```

#### Encoding (`driver/claude/encoding.rs`)

| Action | Bytes |
|--------|-------|
| Nudge | `"{message}\r"` |
| Permission accept | `"y\r"` |
| Permission deny | `"n\r"` |
| AskUser option N | `"{n}\r"` |
| AskUser freeform | `"{text}\r"` |
| Plan accept | Accept keystroke |
| Plan reject | Reject keystroke + `"{feedback}\r"` |

### Codex Driver (`driver/codex/`)

Uses Tier 3 (structured stdout) + Tier 4 (process).

#### State Parsing (`driver/codex/state.rs`)

Parses JSONL from Codex's `--json` stdout:

```rust
fn parse_codex_state(json: &Value) -> Option<AgentState> {
    match json.get("type").and_then(|v| v.as_str()) {
        Some("turn.started") => Some(AgentState::Working),
        Some("turn.completed") => Some(AgentState::WaitingForInput),
        Some("error") => Some(AgentState::Error { detail: "...".into() }),
        Some("thread.completed") => Some(AgentState::Exited { code: Some(0) }),
        _ => None,
    }
}
```

#### Encoding (`driver/codex/encoding.rs`)

| Action | Bytes |
|--------|-------|
| Nudge | `"{message}\r"` |
| Permission accept | `"y\r"` |
| Permission deny | `"n\r"` |
| Question option N | `"{n}\r"` |

### Gemini Driver (`driver/gemini/`)

Uses Tier 1 (hooks) + Tier 2 (session log at `~/.gemini/tmp/<hash>/chats/`)
+ Tier 3 (structured stdout) + Tier 4 (process).

Gemini CLI supports hooks via `settings.json` (project, user, or system
level). Relevant events:
- `AfterTool` — fires after each tool call (like Claude's PostToolUse)
- `AfterAgent` — fires after each agent loop iteration
- `SessionEnd` — fires when the session ends (like Claude's Stop)

Hooks use `"type": "command"` and communicate via stdin/stdout as JSON.
Coop materializes hook config before spawn, same pattern as Claude.

Session logs are stored at `~/.gemini/tmp/<project_hash>/chats/`.
Non-interactive mode uses `--prompt` (like Claude's `--print`).
`--output-format stream-json` produces streaming JSONL.
`--approval-mode yolo` is the equivalent of Claude's
`--dangerously-skip-permissions`.

#### State Parsing (`driver/gemini/state.rs`)

Parses JSONL from Gemini's `--output-format stream-json`:

```rust
fn parse_gemini_state(json: &Value) -> Option<AgentState> {
    if json.get("done").and_then(|v| v.as_bool()) == Some(true) {
        return Some(AgentState::WaitingForInput);
    }
    if json.get("error").is_some() {
        return Some(AgentState::Error { detail: "...".into() });
    }
    Some(AgentState::Working)
}
```

#### Encoding (`driver/gemini/encoding.rs`)

| Action | Bytes |
|--------|-------|
| Nudge | `"{message}\r"` |
| Permission accept | `"y\r"` |
| Permission deny | `"n\r"` |
| Question option N | `"{n}\r"` |

### Unknown Driver (`driver/unknown/`)

Uses Tier 4 (process) + optionally Tier 5 (screen, via `--agent-config`).

Always returns `AgentState::Unknown` unless the child has exited.
`is_nudgeable` returns false. Nudge and respond are unavailable.


## 6. Project Structure

```diagram
coop/
├── Cargo.toml
├── Cargo.lock
├── proto/
│   └── coop/v1/coop.proto
├── src/
│   ├── main.rs                        # CLI, startup
│   ├── config.rs                      # Flags/env config
│   ├── session.rs                     # Select loop
│   ├── pty/
│   │   ├── mod.rs                     # Backend trait
│   │   ├── spawn.rs                   # Native PTY
│   │   ├── attach.rs                  # Tmux + screen compat
│   │   └── nbio.rs                    # Non-blocking I/O
│   ├── screen.rs                      # avt::Vt wrapper
│   ├── ring.rs                        # Ring buffer
│   ├── fanout.rs                      # Broadcast fan-out
│   ├── writer.rs                      # Write lock
│   ├── driver/
│   │   ├── mod.rs                     # Detector/NudgeEncoder/RespondEncoder traits,
│   │   │                              #   AgentState, CompositeDetector
│   │   ├── grace.rs                   # Idle grace timer
│   │   ├── log_watch.rs              # Tier 2 infra: session log file watcher
│   │   ├── jsonl_stdout.rs           # Tier 3 infra: JSONL line parser
│   │   ├── process.rs                # Tier 4: process + PTY activity monitor
│   │   ├── screen_parse.rs           # Tier 5: regex screen parser
│   │   ├── claude/
│   │   │   ├── mod.rs                # Wires up tiers 1+2+(3)+4, implements Driver
│   │   │   ├── state.rs              # parse_claude_state from JSONL
│   │   │   ├── hooks.rs              # Hook pipe setup + receiver
│   │   │   ├── prompt.rs             # Permission, AskUser, Plan context extraction
│   │   │   └── encoding.rs           # Nudge + respond encoding
│   │   ├── codex/
│   │   │   ├── mod.rs                # Wires up tiers 3+4, implements Driver
│   │   │   ├── state.rs              # parse_codex_state from JSONL
│   │   │   └── encoding.rs           # Nudge + respond encoding
│   │   ├── gemini/
│   │   │   ├── mod.rs                # Wires up tiers 1+2+3+4, implements Driver
│   │   │   ├── state.rs              # parse_gemini_state from JSONL
│   │   │   ├── hooks.rs              # Hook config for Gemini CLI
│   │   │   └── encoding.rs           # Nudge + respond encoding
│   │   └── unknown/
│   │       └── mod.rs                # Wires up tiers 4+(5), null driver
│   └── transport/
│       ├── mod.rs                     # Router, shared state
│       ├── http.rs                    # HTTP handlers
│       ├── ws.rs                      # WebSocket handler
│       ├── grpc.rs                    # gRPC service
│       └── auth.rs                    # Token auth middleware
├── tests/
│   ├── integration.rs                 # Spawn coop, hit endpoints
│   ├── claude_detection.rs            # Claude state detection tests
│   ├── codex_detection.rs             # Codex state detection tests
│   └── prompt.rs                      # Prompt context + respond tests
├── Dockerfile
└── deploy/
    ├── k8s-sidecar.yaml
    └── k8s-standalone.yaml
```

### Line Estimates

| Module | Lines (est.) |
|--------|-------------|
| main.rs + config.rs | 240 |
| session.rs | 200 |
| pty/ | 265 |
| screen.rs | 200 |
| ring.rs + fanout.rs + writer.rs | 260 |
| driver/ (shared) | 250 |
| driver/claude/ | 250 |
| driver/codex/ | 80 |
| driver/gemini/ | 120 |
| driver/unknown/ | 30 |
| transport/ | 700 |
| tests/ | 400 |
| **Total** | **~2,950** |

### Dependencies

```toml
[package]
name = "coop"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "coop"
path = "src/main.rs"

[dependencies]
nix = { version = "0.28", features = ["term", "process", "fs", "signal"] }
avt = "0.17"
axum = { version = "0.8", features = ["ws"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "trace"] }
tonic = "0.12"
prost = "0.13"
tokio = { version = "1", features = ["full"] }
clap = { version = "4", features = ["derive", "env"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
base64 = "0.22"
regex = "1"
notify = "7"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
anyhow = "1"
bytes = "1"

[build-dependencies]
tonic-build = "0.12"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
panic = "abort"
```


## 7. CLI Interface

```
coop [OPTIONS] [-- COMMAND [ARGS...]]
coop --attach tmux:SESSION [OPTIONS]
coop --attach screen:SESSION [OPTIONS]
```

### Flags

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--port PORT` | `COOP_PORT` | (none) | TCP port |
| `--socket PATH` | `COOP_SOCKET` | `/var/run/coop.sock` | Unix socket |
| `--host ADDR` | `COOP_HOST` | `0.0.0.0` | Bind address |
| `--grpc-port PORT` | `COOP_GRPC_PORT` | (none) | gRPC port |
| `--auth-token TOKEN` | `COOP_AUTH_TOKEN` | (none) | Bearer token |
| `--agent-type TYPE` | `COOP_AGENT_TYPE` | `unknown` | `claude\|codex\|gemini\|unknown` |
| `--agent-config PATH` | `COOP_AGENT_CONFIG` | (none) | Screen pattern overrides |
| `--idle-grace SECS` | `COOP_IDLE_GRACE` | 60 | Grace timer duration |
| `--attach SPEC` | `COOP_ATTACH` | (none) | `tmux:NAME` or `screen:NAME` |
| `--cols N` | `COOP_COLS` | 200 | Terminal width |
| `--rows N` | `COOP_ROWS` | 50 | Terminal height |
| `--ring-size BYTES` | `COOP_RING_SIZE` | 1048576 | Ring buffer |
| `--term TERM` | `TERM` | `xterm-256color` | TERM for child |
| `--health-port PORT` | `COOP_HEALTH_PORT` | (none) | Health probe port |
| `--idle-timeout SECS` | `COOP_IDLE_TIMEOUT` | 0 | Exit after idle |
| `--log-format FMT` | `COOP_LOG_FORMAT` | json | `json\|text` |
| `--log-level LVL` | `COOP_LOG_LEVEL` | info | Log level |

### Examples

```bash
# Claude with structured detection
coop --agent-type claude --port 8080 -- claude --dangerously-skip-permissions

# Codex on Unix socket
coop --agent-type codex --socket /tmp/coop.sock -- codex

# Dumb PTY server (no driver)
coop --port 8080 -- /bin/bash

# Attach to existing tmux
coop --agent-type claude --attach tmux:gt-alpha --port 8080

# K8s: all transports + auth
coop --agent-type claude --port 8080 --grpc-port 9090 \
  --health-port 9091 --auth-token $TOKEN -- claude

# Poll agent state
curl localhost:8080/api/v1/agent/state

# Nudge an idle agent
curl -X POST localhost:8080/api/v1/agent/nudge \
  -d '{"message": "Fix the login bug"}'

# Accept a permission prompt
curl -X POST localhost:8080/api/v1/agent/respond \
  -d '{"accept": true}'

# Answer a question
curl -X POST localhost:8080/api/v1/agent/respond \
  -d '{"option": 2}'

# Subscribe to state changes
websocat ws://localhost:8080/ws?mode=state
```


## 8. Deployment

### K8s Sidecar

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: gt-gastown-toast
  namespace: gastown
spec:
  shareProcessNamespace: true
  containers:
    - name: agent
      image: gastown-agent:latest
      command: ["sleep", "infinity"]
      env:
        - name: ANTHROPIC_AUTH_TOKEN
          valueFrom:
            secretKeyRef: { name: anthropic-credentials, key: auth-token }
      resources:
        requests: { cpu: "500m", memory: "1Gi" }
        limits: { cpu: "2", memory: "4Gi" }

    - name: coop
      image: coop:latest
      args:
        - "--agent-type=claude"
        - "--port=8080"
        - "--health-port=9090"
        - "--cols=200"
        - "--rows=50"
        - "--auth-token=$(COOP_AUTH_TOKEN)"
        - "--"
        - "claude"
        - "--dangerously-skip-permissions"
      env:
        - name: COOP_AUTH_TOKEN
          valueFrom:
            secretKeyRef: { name: coop-auth, key: token }
        - name: ANTHROPIC_AUTH_TOKEN
          valueFrom:
            secretKeyRef: { name: anthropic-credentials, key: auth-token }
      ports:
        - { containerPort: 8080, name: api }
        - { containerPort: 9090, name: health }
      livenessProbe:
        httpGet: { path: /api/v1/health, port: health }
        initialDelaySeconds: 5
        periodSeconds: 10
      readinessProbe:
        httpGet: { path: /api/v1/agent/state, port: health }
        initialDelaySeconds: 3
        periodSeconds: 5
      startupProbe:
        httpGet: { path: /api/v1/health, port: health }
        failureThreshold: 30
        periodSeconds: 2
      resources:
        requests: { cpu: "50m", memory: "32Mi" }
        limits: { cpu: "200m", memory: "64Mi" }
  restartPolicy: Never
```

### Container Image

```dockerfile
FROM rust:1.84-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl \
    && strip target/x86_64-unknown-linux-musl/release/coop

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=builder /src/target/x86_64-unknown-linux-musl/release/coop /coop
ENTRYPOINT ["/coop"]
```

Target: ~8 MB image.


## 9. Migration

### Phase 1: Coop Alongside Tmux

Validate locally. Existing paths unchanged.

- Ship coop binary
- Opt-in via `--session-backend=coop`
- Gas Town `CoopBackend`:

```go
type CoopBackend struct {
    BaseURL string
    Token   string
    Client  *http.Client
}

func (b *CoopBackend) HasSession(session string) (bool, error) {
    state, _ := b.getAgentState()
    return state.State != "exited", nil
}

func (b *CoopBackend) CapturePane(session string, lines int) (string, error) {
    return b.getText("/api/v1/screen/text")
}

func (b *CoopBackend) NudgeSession(session string, message string) error {
    return b.post("/api/v1/agent/nudge", NudgeRequest{Message: message})
}

func (b *CoopBackend) RespondToPrompt(accept bool) error {
    return b.post("/api/v1/agent/respond", RespondRequest{Accept: &accept})
}

func (b *CoopBackend) AgentState() (*AgentStateResponse, error) {
    return b.get("/api/v1/agent/state")
}
```

**Risk:** Zero. Opt-in.

### Phase 2: K8s Pods Use Coop Sidecar

Replace SSH + tmux + screen with coop sidecar.

**Deletions:**

| File | Lines |
|------|-------|
| `internal/terminal/ssh.go` | 153 |
| `internal/terminal/connection.go` | 265 |
| `deploy/k8s/polecat-ssh-keys.yaml` | all |

**Risk:** Medium. K8s only, new pods only.

### Phase 3: Remove Tmux

All agents use coop.

**Deletions:**

| Package | Lines |
|---------|-------|
| `internal/tmux/` | 1,813 |
| `internal/terminal/local.go` | 33 |
| Gas Town screen parsing | ~300 |
| Oddjobs session log watcher | ~200 |
| Oddjobs idle grace timer | ~100 |

Net: ~2,900 lines replaced by ~60 lines HTTP client per consumer +
coop binary (~2,950 lines).

**Risk:** High. Only after phase 2 stable in production.


## 10. API Reference

### HTTP

| Method | Path | Driver | Description |
|--------|------|--------|-------------|
| GET | `/api/v1/health` | No | Health check |
| GET | `/api/v1/screen` | No | Rendered screen |
| GET | `/api/v1/screen/text` | No | Plain text screen |
| GET | `/api/v1/output` | No | Raw ring buffer |
| GET | `/api/v1/status` | No | Process status |
| POST | `/api/v1/input` | No | Send text input |
| POST | `/api/v1/input/keys` | No | Send key sequences |
| POST | `/api/v1/resize` | No | Resize terminal |
| POST | `/api/v1/signal` | No | Signal child |
| GET | `/api/v1/agent/state` | Yes | Agent state + prompt context |
| POST | `/api/v1/agent/nudge` | Yes* | Deliver message to idle agent |
| POST | `/api/v1/agent/respond` | Yes* | Answer agent prompt |
| GET | `/ws` | No | WebSocket |

*Not available when `--agent-type unknown`.

### WebSocket

| Type | Dir | Driver | Description |
|------|-----|--------|-------------|
| `output` | S→C | No | Raw PTY bytes (base64) |
| `screen` | S→C | No | Rendered screen snapshot |
| `state_change` | S→C | Yes | State transition + prompt context |
| `exit` | S→C | No | Child exited |
| `error` | S→C | No | Error condition |
| `resize` | S→C | No | Terminal resized |
| `input` | C→S | No | Write text to PTY |
| `input_raw` | C→S | No | Write raw bytes |
| `keys` | C→S | No | Send key sequences |
| `resize` | C→S | No | Resize request |
| `screen_request` | C→S | No | Request snapshot |
| `state_request` | C→S | Yes | Request current state |
| `nudge` | C→S | Yes* | Deliver message |
| `respond` | C→S | Yes* | Answer prompt |
| `replay` | C→S | No | Replay from offset |
| `lock` | C→S | No | Write lock control |
| `auth` | C→S | No | Authenticate |
| `ping`/`pong` | Both | No | Keepalive |

### gRPC

| RPC | Driver | Type |
|-----|--------|------|
| `GetHealth` | No | Unary |
| `GetScreen` | No | Unary |
| `GetStatus` | No | Unary |
| `SendInput` | No | Unary |
| `SendKeys` | No | Unary |
| `Resize` | No | Unary |
| `SendSignal` | No | Unary |
| `StreamOutput` | No | Server streaming |
| `StreamScreen` | No | Server streaming |
| `GetAgentState` | Yes | Unary |
| `Nudge` | Yes* | Unary |
| `Respond` | Yes* | Unary |
| `StreamState` | Yes | Server streaming |

### Error Codes

| Code | HTTP | gRPC | Meaning |
|------|------|------|---------|
| `NOT_READY` | 503 | UNAVAILABLE | Child not started |
| `EXITED` | 410 | NOT_FOUND | Child exited |
| `WRITER_BUSY` | 409 | RESOURCE_EXHAUSTED | Write lock held |
| `UNAUTHORIZED` | 401 | UNAUTHENTICATED | Bad auth |
| `BAD_REQUEST` | 400 | INVALID_ARGUMENT | Malformed request |
| `NO_DRIVER` | 404 | UNIMPLEMENTED | Agent endpoint, no driver |
| `AGENT_BUSY` | 409 | FAILED_PRECONDITION | Nudge when not idle |
| `NO_PROMPT` | 409 | FAILED_PRECONDITION | Respond when no prompt |
| `INTERNAL` | 500 | INTERNAL | Backend error |


