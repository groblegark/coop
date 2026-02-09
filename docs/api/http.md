# HTTP REST API

Coop exposes an HTTP REST API for terminal control and agent orchestration.


## Overview

- **Base URL**: `http://localhost:{port}/api/v1`
- **Content-Type**: `application/json` (all request and response bodies)
- **Authentication**: Bearer token via `Authorization` header


## Authentication

When coop is started with `--auth-token <token>` (or `COOP_AUTH_TOKEN` env var),
all endpoints except `/api/v1/health` require a Bearer token:

```
Authorization: Bearer <token>
```

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
| `ready` | bool | Whether the agent is ready for interaction |


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
  "exit_code": null,
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
  "state": "permission_prompt",
  "since_seq": 15,
  "screen_seq": 42,
  "detection_tier": "tier1_hooks",
  "prompt": {
    "prompt_type": "permission",
    "tool": "Bash",
    "input_preview": "{\"command\":\"ls -la\"}",
    "screen_lines": [],
    "questions": [],
    "question_current": 0
  },
  "idle_grace_remaining_secs": null,
  "error_detail": null,
  "error_category": null
}
```

| Field | Type | Description |
|-------|------|-------------|
| `agent` | string | Agent type |
| `state` | string | Current agent state (see table below) |
| `since_seq` | int | Sequence number when this state was entered |
| `screen_seq` | int | Current screen sequence number |
| `detection_tier` | string | Which detection tier produced this state |
| `prompt` | object or null | Prompt context (present for prompt states) |
| `idle_grace_remaining_secs` | float or null | Seconds remaining on the idle grace timer |
| `error_detail` | string or null | Error description (when state is `error`) |
| `error_category` | string or null | Error classification (when state is `error`) |

**Agent states:**

| State | Description |
|-------|-------------|
| `starting` | Initial state before first detection |
| `working` | Executing tool calls or thinking |
| `waiting_for_input` | Idle, ready for a nudge |
| `permission_prompt` | Requesting tool permission (has `prompt`) |
| `plan_prompt` | Presenting a plan for approval (has `prompt`) |
| `question` | Multi-question dialog (has `prompt`) |
| `error` | Error occurred (has `error_detail`) |
| `alt_screen` | Alternate screen buffer active |
| `exited` | Child process exited |
| `unknown` | State not yet determined |

**PromptContext shape:**

| Field | Type | Description |
|-------|------|-------------|
| `prompt_type` | string | `"permission"`, `"plan"`, `"question"` |
| `tool` | string or null | Tool name (permission prompts) |
| `input_preview` | string or null | Truncated tool input (permission prompts) |
| `screen_lines` | string[] | Raw screen lines (plan prompts) |
| `questions` | QuestionContext[] | All questions in a multi-question dialog |
| `question_current` | int | 0-indexed current question; equals `questions.len()` at confirm phase |

**QuestionContext shape:**

| Field | Type | Description |
|-------|------|-------------|
| `question` | string | The question text |
| `options` | string[] | Available option labels |


### `POST /api/v1/agent/nudge`

Send a follow-up message to the agent. Only succeeds when the agent is in
`waiting_for_input` state.

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
  "state_before": "waiting_for_input",
  "reason": null
}
```

**Response (not delivered):**

```json
{
  "delivered": false,
  "state_before": "working",
  "reason": "agent_busy"
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

Respond to an active prompt. Behavior depends on the current agent state.

**Request:**

```json
{
  "accept": true,
  "text": null,
  "answers": []
}
```

| Field | Type | Description |
|-------|------|-------------|
| `accept` | bool or null | Accept/deny (permission and plan prompts) |
| `text` | string or null | Freeform text (plan rejection feedback) |
| `answers` | QuestionAnswer[] | Structured answers for multi-question dialogs |

**QuestionAnswer shape:**

| Field | Type | Description |
|-------|------|-------------|
| `option` | int or null | 1-indexed option number |
| `text` | string or null | Freeform text (used when selecting "Other") |

**Per-prompt behavior:**

| Agent state | Fields used | Action |
|-------------|-------------|--------|
| `permission_prompt` | `accept` | Accept (`true`) or deny (`false`) the tool call |
| `plan_prompt` | `accept`, `text` | Accept the plan, or reject with optional feedback in `text` |
| `question` | `answers` | Answer one or more questions in the dialog |

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
  "prompt_type": "permission_prompt",
  "reason": null
}
```

| Field | Type | Description |
|-------|------|-------------|
| `delivered` | bool | Whether the response was written to the PTY |
| `prompt_type` | string or null | Agent state at the time of the request |
| `reason` | string or null | Why the response was not delivered |

**Errors:**
- `NOT_READY` (503) -- agent is still starting
- `NO_DRIVER` (404) -- no agent driver configured
- `NO_PROMPT` (409) -- agent is not in a prompt state
