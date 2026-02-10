# WebSocket Protocol

Coop provides a WebSocket endpoint for real-time terminal output streaming,
agent state changes, and bidirectional control.


## Overview

- **URL**: `ws://localhost:{port}/ws`
- **Query parameters**: `mode` (subscription mode), `token` (auth token)
- **Protocol**: JSON text frames, one message per frame
- **Message format**: Internally-tagged JSON (`{"type": "...", ...}`)


## Authentication

WebSocket connections have two authentication paths:

1. **Query parameter** -- pass `?token=<token>` on the upgrade request
2. **Auth message** -- send a `{"type": "auth", "token": "..."}` message after connecting

When `--auth-token` is configured, the WebSocket upgrade always succeeds
(the `/ws` path skips HTTP auth middleware). If no token is provided in the
query string, the connection starts in an unauthenticated state.

**Per-connection auth state:**

| Token in query | Auth state | Read operations | Write operations |
|----------------|------------|-----------------|------------------|
| Valid | Authenticated | Allowed | Allowed |
| Missing | Unauthenticated | Allowed | Blocked until `auth` message |
| Invalid | Rejected | Connection refused (401) | -- |

Read-only operations (subscriptions, `screen_request`, `state_request`,
`status_request`, `replay`, `ping`) are always available. Write operations
(`input`, `input_raw`, `keys`, `nudge`, `respond`, `signal`, `shutdown`)
require authentication. `resize` does not require authentication.


## Subscription Modes

Set via the `mode` query parameter on the upgrade URL.

| Mode | Query value | Server pushes |
|------|-------------|---------------|
| Raw output | `raw` | `output` messages with base64-encoded PTY bytes |
| Screen updates | `screen` | `screen` messages with rendered terminal state |
| State changes | `state` | `state_change`, `exit`, `stop`, and `start` messages |
| All (default) | `all` | All of the above |

Example: `ws://localhost:8080/ws?mode=screen&token=mytoken`


## Server → Client Messages


### `output`

Raw PTY output chunk. Sent in `raw` and `all` modes.

```json
{
  "type": "output",
  "data": "SGVsbG8gV29ybGQ=",
  "offset": 1024
}
```

| Field | Type | Description |
|-------|------|-------------|
| `data` | string | Base64-encoded raw bytes |
| `offset` | int | Byte offset in the output stream |


### `screen`

Rendered terminal screen snapshot. Sent in `screen` and `all` modes on each
screen update, or in response to a `screen_request`.

```json
{
  "type": "screen",
  "lines": ["$ hello", "world", ""],
  "cols": 120,
  "rows": 40,
  "alt_screen": false,
  "cursor": { "row": 2, "col": 0 },
  "seq": 42
}
```

| Field | Type | Description |
|-------|------|-------------|
| `lines` | string[] | One string per terminal row |
| `cols` | int | Terminal width |
| `rows` | int | Terminal height |
| `alt_screen` | bool | Whether the alternate screen buffer is active |
| `cursor` | CursorPosition or null | Cursor position |
| `seq` | int | Monotonic screen sequence number |


### `state_change`

Agent state transition. Sent in `state` and `all` modes, or in response
to a `state_request`.

```json
{
  "type": "state_change",
  "prev": "working",
  "next": "prompt",
  "seq": 15,
  "prompt": {
    "type": "permission",
    "subtype": "tool",
    "tool": "Bash",
    "input": "{\"command\":\"rm -rf /tmp/test\"}",
    "options": ["Yes", "Yes, and don't ask again for this tool", "No"],
    "options_fallback": false,
    "question_current": 0,
    "ready": true
  },
  "cause": "tier1_hooks"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `prev` | string | Previous agent state |
| `next` | string | New agent state |
| `seq` | int | State sequence number |
| `prompt` | PromptContext or null | Prompt context (when `next` is `"prompt"`) |
| `error_detail` | string or null | Error text (when `next` is `"error"`) |
| `error_category` | string or null | Error classification (when `next` is `"error"`) |
| `cause` | string | Detection source that triggered this transition |
| `last_message` | string or null | Last message extracted from agent output |


### `exit`

Agent process exited. Sent in `state` and `all` modes. This replaces
`state_change` for the terminal `exited` state.

```json
{
  "type": "exit",
  "code": 0
}
```

| Field | Type | Description |
|-------|------|-------------|
| `code` | int or null | Process exit code |
| `signal` | int or null | Signal number that killed the process |


### `nudge_result`

Result of a `nudge` request. Always sent in response to a client `nudge`.

```json
{
  "type": "nudge_result",
  "delivered": true,
  "state_before": "idle"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `delivered` | bool | Whether the nudge was written to the PTY |
| `state_before` | string or null | Agent state at the time of the request |
| `reason` | string or null | Why the nudge was not delivered |


### `respond_result`

Result of a `respond` request. Always sent in response to a client `respond`.

```json
{
  "type": "respond_result",
  "delivered": true,
  "prompt_type": "permission"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `delivered` | bool | Whether the response was written to the PTY |
| `prompt_type` | string or null | Prompt type at the time of the request |
| `reason` | string or null | Why the response was not delivered |


### `status`

Session status summary. Sent in response to a `status_request`.

```json
{
  "type": "status",
  "state": "running",
  "pid": 12345,
  "uptime_secs": 120,
  "screen_seq": 42,
  "bytes_read": 8192,
  "bytes_written": 256,
  "ws_clients": 2
}
```

| Field | Type | Description |
|-------|------|-------------|
| `state` | string | `"starting"`, `"running"`, or `"exited"` |
| `pid` | int or null | Child process PID |
| `uptime_secs` | int | Seconds since coop started |
| `exit_code` | int or null | Exit code if exited |
| `screen_seq` | int | Current screen sequence number |
| `bytes_read` | int | Total bytes read from PTY |
| `bytes_written` | int | Total bytes written to PTY |
| `ws_clients` | int | Connected WebSocket clients |


### `stop`

Stop hook verdict event. Sent in `state` and `all` modes whenever a stop
hook check occurs.

```json
{
  "type": "stop",
  "stop_type": "blocked",
  "seq": 0
}
```

| Field | Type | Description |
|-------|------|-------------|
| `stop_type` | string | Verdict type (see table below) |
| `signal` | JSON or null | Signal body (when `stop_type` is `"signaled"`) |
| `error_detail` | string or null | Error details (when `stop_type` is `"error"`) |
| `seq` | int | Monotonic stop event sequence number |

**Stop types:**

| Type | Description |
|------|-------------|
| `signaled` | Signal received via resolve endpoint; agent allowed to stop |
| `error` | Agent in unrecoverable error state; allowed to stop |
| `safety_valve` | Claude's safety valve triggered; must allow |
| `blocked` | Stop was blocked; agent should continue working |
| `allowed` | Mode is `allow`; agent always allowed to stop |


### `start`

Start hook event. Sent in `state` and `all` modes whenever a session
lifecycle event fires.

```json
{
  "type": "start",
  "source": "resume",
  "session_id": "abc123",
  "injected": true,
  "seq": 0
}
```

| Field | Type | Description |
|-------|------|-------------|
| `source` | string | Lifecycle event type (e.g. `"start"`, `"resume"`, `"clear"`) |
| `session_id` | string or null | Session identifier if available |
| `injected` | bool | Whether a non-empty script was injected |
| `seq` | int | Monotonic start event sequence number |


### `error`

Error response to a client message.

```json
{
  "type": "error",
  "code": "BAD_REQUEST",
  "message": "unknown key: badkey"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `code` | string | Error code (same codes as HTTP API) |
| `message` | string | Human-readable error description |


### `resize`

Terminal resize notification. Sent when the PTY is resized.

```json
{
  "type": "resize",
  "cols": 120,
  "rows": 40
}
```


### `pong`

Response to a client `ping`.

```json
{
  "type": "pong"
}
```


## Client → Server Messages


### `ping`

Keepalive ping. No auth required.

```json
{
  "type": "ping"
}
```

Server replies with `pong`.


### `auth`

Authenticate an unauthenticated connection. No auth required (this is the
auth mechanism itself).

```json
{
  "type": "auth",
  "token": "my-secret-token"
}
```

On success: no response (connection is now authenticated).
On failure: `error` message with code `UNAUTHORIZED`.


### `screen_request`

Request the current screen snapshot. No auth required.

```json
{
  "type": "screen_request"
}
```

Server replies with a `screen` message.


### `state_request`

Request the current agent state. No auth required.

```json
{
  "type": "state_request"
}
```

Server replies with a `state_change` message where `prev` and `next` are the
same (representing current state, not a transition).


### `status_request`

Request the current session status. No auth required.

```json
{
  "type": "status_request"
}
```

Server replies with a `status` message.


### `replay`

Request raw output from a specific byte offset. No auth required.

```json
{
  "type": "replay",
  "offset": 0
}
```

| Field | Type | Description |
|-------|------|-------------|
| `offset` | int | Byte offset to start reading from |

Server replies with an `output` message containing the buffered data.


### `input`

Write UTF-8 text to the PTY. **Requires auth.**

```json
{
  "type": "input",
  "text": "hello",
  "enter": true
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `text` | string | required | Text to write to the PTY |
| `enter` | bool | `false` | Append carriage return (`\r`) after text |

No response on success. Error on auth failure.


### `input_raw`

Write base64-encoded raw bytes to the PTY. **Requires auth.**

```json
{
  "type": "input_raw",
  "data": "SGVsbG8="
}
```

| Field | Type | Description |
|-------|------|-------------|
| `data` | string | Base64-encoded bytes |

No response on success.


### `keys`

Send named key sequences to the PTY. **Requires auth.**

```json
{
  "type": "keys",
  "keys": ["ctrl-c", "enter"]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `keys` | string[] | Key names (see HTTP API key table for supported names) |

No response on success. Error with `BAD_REQUEST` if a key name is unrecognized.


### `resize`

Resize the PTY. No auth required.

```json
{
  "type": "resize",
  "cols": 120,
  "rows": 40
}
```

| Field | Type | Description |
|-------|------|-------------|
| `cols` | int | New column count (must be > 0) |
| `rows` | int | New row count (must be > 0) |

No response on success. Error with `BAD_REQUEST` if dimensions are zero.


### `nudge`

Send a follow-up message to the agent. **Requires auth.**
Only succeeds when the agent is in `idle` state.

```json
{
  "type": "nudge",
  "message": "Please continue"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `message` | string | Text message to send to the agent |

Server replies with a `nudge_result`. Error on auth failure or if no
agent driver is configured.


### `respond`

Respond to an active prompt. **Requires auth.** Behavior depends on the current
agent state.

```json
{
  "type": "respond",
  "accept": true
}
```

| Field | Type | Description |
|-------|------|-------------|
| `accept` | bool or null | Accept/deny (permission and plan prompts). Overridden by `option` when set |
| `option` | int or null | 1-indexed option number for permission/plan/setup prompts |
| `text` | string or null | Freeform text (plan feedback) |
| `answers` | QuestionAnswer[] | Structured answers for multi-question dialogs |

See the HTTP API `POST /api/v1/agent/respond` for per-prompt behavior.

Server replies with a `respond_result`. Error on auth failure or if no agent
driver is configured.


### `signal`

Send a signal to the child process. **Requires auth.**

```json
{
  "type": "signal",
  "signal": "SIGINT"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `signal` | string | Signal name or number (see HTTP API signal table) |

No response on success. Error with `BAD_REQUEST` if the signal is unrecognized.


### `shutdown`

Initiate graceful shutdown of the coop process. **Requires auth.**

```json
{
  "type": "shutdown"
}
```

No response on success. The connection will close as the server shuts down.


## Shared Types


### CursorPosition

```json
{
  "row": 0,
  "col": 0
}
```

| Field | Type | Description |
|-------|------|-------------|
| `row` | int | 0-indexed row |
| `col` | int | 0-indexed column |


### PromptContext

```json
{
  "type": "permission",
  "subtype": "tool",
  "tool": "Bash",
  "input": "{\"command\":\"ls\"}",
  "options": ["Yes", "Yes, and don't ask again for this tool", "No"],
  "options_fallback": false,
  "question_current": 0,
  "ready": true
}
```

All `Option` fields are omitted when null; `Vec` fields are omitted when empty.

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | Prompt type: `"permission"`, `"plan"`, `"question"`, `"setup"` |
| `subtype` | string or null | Further classification (see HTTP API for known subtypes) |
| `tool` | string or null | Tool name (permission prompts) |
| `input` | string or null | Truncated tool input JSON (permission prompts) |
| `auth_url` | string or null | OAuth authorization URL (setup `oauth_login` prompts) |
| `options` | string[] | Numbered option labels parsed from the terminal screen |
| `options_fallback` | bool | True when options are fallback labels (parser couldn't find real ones) |
| `questions` | QuestionContext[] | All questions in a multi-question dialog |
| `question_current` | int | 0-indexed current question; equals `questions.len()` at confirm phase |
| `ready` | bool | True when all async enrichment (e.g. option parsing) is complete |


### QuestionContext

```json
{
  "question": "Which database should we use?",
  "options": ["PostgreSQL", "SQLite", "MySQL"]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `question` | string | The question text |
| `options` | string[] | Available option labels |


### QuestionAnswer

```json
{
  "option": 1
}
```

All fields are omitted when null.

| Field | Type | Description |
|-------|------|-------------|
| `option` | int or null | 1-indexed option number |
| `text` | string or null | Freeform text (used when selecting "Other") |
