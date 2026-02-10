# HTTP REST API

Coop exposes an HTTP REST API for terminal control and agent orchestration.


## Overview

- **Base URL**: `http://localhost:{port}/api/v1`
- **Content-Type**: `application/json` (all request and response bodies)
- **Authentication**: Bearer token via `Authorization` header


## Authentication

When coop is started with `--auth-token <token>` (or `COOP_AUTH_TOKEN` env var),
all endpoints require a Bearer token except those marked **No authentication required**:

```
Authorization: Bearer <token>
```

**Auth-exempt paths:** `/api/v1/health`, `/api/v1/hooks/stop`,
`/api/v1/hooks/stop/resolve`, `/api/v1/hooks/start`, and `/ws` (WebSocket
handles auth separately via query param or Auth message).

Unauthenticated requests receive a `401` response:

```json
{
  "error": {
    "code": "UNAUTHORIZED",
    "message": "unauthorized"
  }
}
```


## Error Responses

All errors use a standard envelope:

```json
{
  "error": {
    "code": "ERROR_CODE",
    "message": "Human-readable description"
  }
}
```

| Code | HTTP Status | Meaning |
|------|-------------|---------|
| `UNAUTHORIZED` | 401 | Missing or invalid auth token |
| `BAD_REQUEST` | 400 | Invalid request body or parameters |
| `NO_DRIVER` | 404 | Agent driver not configured (missing `--agent`) |
| `NOT_READY` | 503 | Agent still starting up |
| `AGENT_BUSY` | 409 | Agent is not in the expected state for this operation |
| `NO_PROMPT` | 409 | No active prompt to respond to |
| `EXITED` | 410 | Agent process has exited |
| `INTERNAL` | 500 | Internal server error |


## Terminal Endpoints

These endpoints are always available regardless of `--agent` flag.


### `GET /api/v1/health`

Health check. **No authentication required.**

**Response:**

```json
{
  "status": "running",
  "pid": 12345,
  "uptime_secs": 120,
  "agent": "claude",
  "terminal": {
    "cols": 120,
    "rows": 40
  },
  "ws_clients": 2,
  "ready": true
}
```

| Field | Type | Description |
|-------|------|-------------|
| `status` | string | Always `"running"` |
| `pid` | int or null | Child process PID, null if not yet spawned |
| `uptime_secs` | int | Seconds since coop started |
| `agent` | string | Agent type (`"claude"`, `"codex"`, `"gemini"`, `"unknown"`) |
| `terminal` | object | Current terminal dimensions |
| `ws_clients` | int | Number of connected WebSocket clients |
| `ready` | bool | Whether the session is ready (agent has left starting state) |


### `GET /api/v1/ready`

Readiness probe. Returns `200` when ready, `503` when not.

**Response:**

```json
{
  "ready": true
}
```


### `GET /api/v1/screen`

Rendered terminal screen content.

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `format` | string | `"text"` | `"text"` for plain text, `"ansi"` for ANSI escape sequences |
| `cursor` | bool | `false` | Include cursor position in response |

**Response:**

```json
{
  "lines": ["$ hello world", "output line 1", ""],
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
| `cursor` | object or null | Cursor position (only when `cursor=true`) |
| `seq` | int | Monotonic screen update sequence number |


### `GET /api/v1/screen/text`

Plain text screen dump. Returns `text/plain` instead of JSON.

**Response:** Newline-joined terminal lines as plain text.


### `GET /api/v1/output`

Raw PTY output bytes from the ring buffer, base64-encoded.

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `offset` | int | `0` | Byte offset to start reading from |
| `limit` | int | none | Maximum number of bytes to return |

**Response:**

```json
{
  "data": "SGVsbG8gV29ybGQ=",
  "offset": 0,
  "next_offset": 11,
  "total_written": 1024
}
```

| Field | Type | Description |
|-------|------|-------------|
| `data` | string | Base64-encoded raw output bytes |
| `offset` | int | Requested start offset |
| `next_offset` | int | Offset for the next read (`offset + bytes returned`) |
| `total_written` | int | Total bytes written to the ring buffer since start |

Use `next_offset` as the `offset` parameter in subsequent calls to stream output incrementally.


### `GET /api/v1/status`

Session status summary.

**Response:**

```json
{
  "state": "running",
  "pid": 12345,
  "uptime_secs": 120,
  "screen_seq": 42,
  "bytes_read": 8192,
  "bytes_written": 256,
  "ws_clients": 1
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


### `POST /api/v1/input`

Write text to the PTY.

**Request:**

```json
{
  "text": "hello",
  "enter": true
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `text` | string | required | Text to write |
| `enter` | bool | `false` | Append carriage return (`\r`) after text |

**Response:**

```json
{
  "bytes_written": 6
}
```


### `POST /api/v1/input/raw`

Write base64-encoded raw bytes to the PTY.

**Request:**

```json
{
  "data": "SGVsbG8="
}
```

| Field | Type | Description |
|-------|------|-------------|
| `data` | string | Base64-encoded bytes to write |

**Response:**

```json
{
  "bytes_written": 5
}
```

**Errors:** `BAD_REQUEST` if `data` is not valid base64.


### `POST /api/v1/input/keys`

Send named key sequences to the PTY.

**Request:**

```json
{
  "keys": ["ctrl-c", "enter"]
}
```

**Response:**

```json
{
  "bytes_written": 2
}
```

**Supported key names** (case-insensitive):

| Key | Alias |
|-----|-------|
| `enter` | `return` |
| `tab` | |
| `escape` | `esc` |
| `backspace` | |
| `delete` | `del` |
| `space` | |
| `up` | |
| `down` | |
| `right` | |
| `left` | |
| `home` | |
| `end` | |
| `pageup` | `page_up` |
| `pagedown` | `page_down` |
| `insert` | |
| `f1` .. `f12` | |
| `ctrl-{a..z}` | |

**Errors:** `BAD_REQUEST` if any key name is unrecognized.


### `POST /api/v1/resize`

Resize the PTY.

**Request:**

```json
{
  "cols": 120,
  "rows": 40
}
```

| Field | Type | Description |
|-------|------|-------------|
| `cols` | int | New column count (must be > 0) |
| `rows` | int | New row count (must be > 0) |

**Response:**

```json
{
  "cols": 120,
  "rows": 40
}
```

**Errors:** `BAD_REQUEST` if `cols` or `rows` is zero.


### `POST /api/v1/signal`

Send a signal to the child process.

**Request:**

```json
{
  "signal": "SIGINT"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `signal` | string | Signal name or number |

**Response:**

```json
{
  "delivered": true
}
```

**Supported signals:**

| Name | Number | Aliases |
|------|--------|---------|
| HUP | 1 | SIGHUP |
| INT | 2 | SIGINT |
| QUIT | 3 | SIGQUIT |
| KILL | 9 | SIGKILL |
| USR1 | 10 | SIGUSR1 |
| USR2 | 12 | SIGUSR2 |
| TERM | 15 | SIGTERM |
| CONT | 18 | SIGCONT |
| STOP | 19 | SIGSTOP |
| TSTP | 20 | SIGTSTP |
| WINCH | 28 | SIGWINCH |

Signal names are case-insensitive and accept bare names (`INT`), prefixed names
(`SIGINT`), or numeric strings (`2`).

**Errors:** `BAD_REQUEST` if the signal name is unrecognized.


## Agent Endpoints

These endpoints require the `--agent` flag. Without it, they return `NO_DRIVER`.


### `GET /api/v1/agent/state`

Current agent state and prompt context.

**Response:**

```json
{
  "agent": "claude",
  "state": "prompt",
  "since_seq": 15,
  "screen_seq": 42,
  "detection_tier": "tier1_hooks",
  "prompt": {
    "type": "permission",
    "subtype": "tool",
    "tool": "Bash",
    "input": "{\"command\":\"ls -la\"}",
    "options": ["Yes", "Yes, and don't ask again for this tool", "No"],
    "options_fallback": false,
    "question_current": 0,
    "ready": true
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `agent` | string | Agent type |
| `state` | string | Current agent state (see table below) |
| `since_seq` | int | Sequence number when this state was entered |
| `screen_seq` | int | Current screen sequence number |
| `detection_tier` | string | Which detection tier produced this state |
| `prompt` | PromptContext or null | Prompt context (present when state is `"prompt"`) |
| `error_detail` | string or null | Error description (when state is `"error"`) |
| `error_category` | string or null | Error classification (when state is `"error"`) |
| `last_message` | string or null | Last message extracted from agent output |

**Agent states:**

| State | Description |
|-------|-------------|
| `starting` | Initial state before first detection |
| `working` | Executing tool calls or thinking |
| `idle` | Idle, ready for a nudge |
| `prompt` | Presenting a prompt (see `prompt.type` for kind) |
| `error` | Error occurred (has `error_detail`) |
| `exited` | Child process exited |
| `unknown` | State not yet determined |

**Error categories** (values of `error_category` when state is `"error"`):

| Category | Description |
|----------|-------------|
| `unauthorized` | Authentication or API key error |
| `out_of_credits` | Billing or credit limit reached |
| `rate_limited` | Rate limit exceeded |
| `no_internet` | Network connectivity issue |
| `server_error` | Upstream API server error |
| `other` | Unclassified error |


### `POST /api/v1/agent/nudge`

Send a follow-up message to the agent. Only succeeds when the agent is in
`idle` state.

**Request:**

```json
{
  "message": "Please continue with the implementation"
}
```

**Response (delivered):**

```json
{
  "delivered": true,
  "state_before": "idle"
}
```

**Response (not delivered):**

```json
{
  "delivered": false,
  "state_before": "working",
  "reason": "agent is working"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `delivered` | bool | Whether the nudge was written to the PTY |
| `state_before` | string or null | Agent state at the time of the request |
| `reason` | string or null | Why the nudge was not delivered |

**Errors:**
- `NOT_READY` (503) -- agent is still starting
- `NO_DRIVER` (404) -- no agent driver configured


### `POST /api/v1/agent/respond`

Respond to an active prompt. Fields used depend on prompt type.

**Request:**

```json
{
  "accept": true
}
```

| Field | Type | Description |
|-------|------|-------------|
| `accept` | bool or null | Accept/deny (permission and plan prompts). Overridden by `option` when set |
| `option` | int or null | 1-indexed option number for permission/plan/setup prompts |
| `text` | string or null | Freeform text (plan feedback) |
| `answers` | QuestionAnswer[] | Structured answers for multi-question dialogs |

**Per-prompt behavior:**

| Prompt type | Fields used | Action |
|-------------|-------------|--------|
| `permission` | `accept` or `option` | Accept or deny the tool call |
| `plan` | `accept` or `option`, `text` | Accept the plan, or reject with optional feedback |
| `question` | `answers` | Answer one or more questions in the dialog |
| `setup` | `option` | Select a setup option (defaults to 1) |

**Multi-question flow:**

When the agent presents multiple questions (`questions.len() > 1`), use the
`answers` array to provide responses. Each answer in the array corresponds to
the next unanswered question starting from `question_current`. After delivery,
`question_current` advances by the number of answers provided. Poll
`GET /api/v1/agent/state` to track progress.

**Response:**

```json
{
  "delivered": true,
  "prompt_type": "permission"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `delivered` | bool | Whether the response was written to the PTY |
| `prompt_type` | string or null | Prompt type at the time of the request |
| `reason` | string or null | Why the response was not delivered |

**Errors:**
- `NOT_READY` (503) -- agent is still starting
- `NO_DRIVER` (404) -- no agent driver configured
- `NO_PROMPT` (409) -- agent is not in a prompt state


## Stop Hook Endpoints

The hook script calls `hooks/stop` and receives a verdict (allow or block).
The resolve endpoint unblocks pending stops.

**No authentication required** -- called from inside the PTY by hook scripts.


### `POST /api/v1/hooks/stop`

Called by the stop hook script. Returns a verdict.

**Request:**

```json
{
  "event": "stop",
  "data": {
    "stop_hook_active": false
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `event` | string | Event name (always `"stop"`) |
| `data` | object or null | Hook data payload |
| `data.stop_hook_active` | bool | When `true`, this is a safety-valve invocation that must be allowed |

**Response (allow):**

```json
{
  "last_message": "Completed the refactoring task"
}
```

**Response (block):**

```json
{
  "decision": "block",
  "reason": "When ready to stop, run: `coop send '{...}'`",
  "last_message": "Completed the refactoring task"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `decision` | string or null | `"block"` when blocked, absent when allowed |
| `reason` | string or null | Instructions for the agent (when blocked) |
| `last_message` | string or null | Last message from agent output |

**Verdict logic:**

1. Mode is `allow` → always allow
2. `stop_hook_active` is `true` → safety valve, must allow
3. Agent has an unrecoverable error (unauthorized, out of credits) → allow
4. Signal has been received via resolve endpoint → allow and reset
5. Otherwise → block


### `POST /api/v1/hooks/stop/resolve`

Resolve a pending stop gate so the agent is allowed to stop. Stores the
JSON body as the signal payload.

**Request:** Any valid JSON body.

```json
{
  "status": "done",
  "message": "Task completed successfully"
}
```

**Response:**

```json
{
  "accepted": true
}
```


### `GET /api/v1/config/stop`

Read the current stop hook configuration.

**Response:**

```json
{
  "mode": "signal",
  "prompt": "Before stopping, summarize what you accomplished.",
  "schema": {
    "fields": {
      "status": {
        "required": true,
        "enum": ["done", "continue"],
        "descriptions": {
          "done": "Work is complete",
          "continue": "Still working, need more time"
        }
      },
      "message": {
        "required": true,
        "description": "Summary of work done"
      }
    }
  }
}
```

See [StopConfig](#stopconfig) for the full type definition.


### `PUT /api/v1/config/stop`

Update the stop hook configuration.

**Request:** A `StopConfig` JSON object.

**Response:**

```json
{
  "updated": true
}
```


## Start Hook Endpoints

Context injection on session lifecycle events (startup, resume, clear, compact).
The hook script calls `hooks/start` and evaluates the returned shell script.

**No authentication required** -- called from inside the PTY by hook scripts.


### `POST /api/v1/hooks/start`

Called by the start hook script. Returns a shell script to evaluate.

**Request:**

```json
{
  "event": "start",
  "data": {
    "source": "resume",
    "session_id": "abc123"
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `event` | string | Event name (always `"start"`) |
| `data` | object or null | Hook data payload |
| `data.source` | string | Lifecycle event type (e.g. `"start"`, `"resume"`, `"clear"`) |
| `data.session_id` | string or null | Session identifier if available |

**Response:** `text/plain` shell script to evaluate. May be empty if no
injection is configured for this event source.


### `GET /api/v1/config/start`

Read the current start hook configuration.

**Response:**

```json
{
  "text": "Remember: always run tests before committing.",
  "shell": [],
  "event": {
    "resume": {
      "text": "Welcome back. Continue where you left off.",
      "shell": []
    }
  }
}
```

See [StartConfig](#startconfig) for the full type definition.


### `PUT /api/v1/config/start`

Update the start hook configuration.

**Request:** A `StartConfig` JSON object.

**Response:**

```json
{
  "updated": true
}
```


## Lifecycle Endpoints


### `POST /api/v1/shutdown`

Initiate graceful shutdown of the coop process.

**Response:**

```json
{
  "accepted": true
}
```


## Shared Types


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
| `subtype` | string or null | Further classification (see below) |
| `tool` | string or null | Tool name (permission prompts) |
| `input` | string or null | Truncated tool input JSON (permission prompts) |
| `auth_url` | string or null | OAuth authorization URL (setup `oauth_login` prompts) |
| `options` | string[] | Numbered option labels parsed from the terminal screen |
| `options_fallback` | bool | True when options are fallback labels (parser couldn't find real ones) |
| `questions` | QuestionContext[] | All questions in a multi-question dialog |
| `question_current` | int | 0-indexed current question; equals `questions.len()` at confirm phase |
| `ready` | bool | True when all async enrichment (e.g. option parsing) is complete |

**Known subtypes by prompt type:**

| Kind | Subtypes |
|------|----------|
| `permission` | `"trust"` (workspace trust), `"tool"` (tool permission) |
| `setup` | `"theme_picker"`, `"terminal_setup"`, `"security_notes"`, `"login_success"`, `"login_method"`, `"oauth_login"` |
| `plan` | (none currently) |
| `question` | (none currently) |


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


### StopConfig

```json
{
  "mode": "signal",
  "prompt": "Custom prompt text",
  "schema": {
    "fields": {
      "status": {
        "required": true,
        "enum": ["done", "continue"],
        "descriptions": { "done": "Work complete", "continue": "Still working" },
        "description": "Current status"
      }
    }
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `mode` | string | `"allow"` (always allow stops) or `"signal"` (block until signaled) |
| `prompt` | string or null | Custom prompt text included in the block reason |
| `schema` | object or null | Schema describing expected signal body fields |
| `schema.fields` | object | Map of field name → field definition |
| `schema.fields.*.required` | bool | Whether this field is required |
| `schema.fields.*.enum` | string[] or null | Allowed values |
| `schema.fields.*.descriptions` | object or null | Per-value descriptions for enum fields |
| `schema.fields.*.description` | string or null | Field-level description |


### StartConfig

```json
{
  "text": "Static text to inject",
  "shell": ["echo hello"],
  "event": {
    "resume": {
      "text": "Override text for resume events",
      "shell": []
    }
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `text` | string or null | Static text to inject (delivered as base64-decoded printf) |
| `shell` | string[] | Shell commands to run |
| `event` | object | Per-event overrides keyed by source (e.g. `"clear"`, `"resume"`) |
| `event.*.text` | string or null | Override text for this event type |
| `event.*.shell` | string[] | Override shell commands for this event type |
