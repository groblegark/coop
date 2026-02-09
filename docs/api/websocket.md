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
| Missing | Unauthenticated | Allowed | Blocked until `Auth` message |
| Invalid | Rejected | Connection refused (401) | -- |

Read-only operations (subscriptions, `screen_request`, `state_request`, `replay`,
`ping`) are always available. Write operations (`input`, `input_raw`, `keys`,
`nudge`, `respond`, `signal`) require authentication.


## Subscription Modes

Set via the `mode` query parameter on the upgrade URL.

| Mode | Query value | Server pushes |
|------|-------------|---------------|
| Raw output | `raw` | `output` messages with base64-encoded PTY bytes |
| Screen updates | `screen` | `screen` messages with rendered terminal state |
| State changes | `state` | `state_change` and `exit` messages |
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
  "next": "permission_prompt",
  "seq": 15,
  "prompt": {
    "prompt_type": "permission",
    "tool": "Bash",
    "input": "{\"command\":\"rm -rf /tmp/test\"}",
    "questions": [],
    "question_current": 0
  },
  "error_detail": null,
  "error_category": null
}
```

| Field | Type | Description |
|-------|------|-------------|
| `prev` | string | Previous agent state |
| `next` | string | New agent state |
| `seq` | int | State sequence number |
| `prompt` | PromptContext or null | Prompt context (for prompt states) |
| `error_detail` | string or null | Error text (when `next` is `"error"`) |
| `error_category` | string or null | Error classification (when `next` is `"error"`) |


### `exit`

Agent process exited. Sent in `state` and `all` modes. This replaces
`state_change` for the terminal `exited` state.

```json
{
  "type": "exit",
  "code": 0,
  "signal": null
}
```

| Field | Type | Description |
|-------|------|-------------|
| `code` | int or null | Process exit code |
| `signal` | int or null | Signal number that killed the process |


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
  "text": "hello\n"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `text` | string | Text to write to the PTY |

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

Resize the PTY. Auth not required.

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
Only succeeds when the agent is in `waiting_for_input` state.

```json
{
  "type": "nudge",
  "message": "Please continue"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `message` | string | Text message to send to the agent |

No response on success. Error with `AGENT_BUSY` if the agent is not waiting.


### `respond`

Respond to an active prompt. **Requires auth.** Behavior depends on the current
agent state.

```json
{
  "type": "respond",
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

See the HTTP API `POST /api/v1/agent/respond` documentation for per-prompt
behavior and multi-question flow details.

No response on success. Error with `NO_PROMPT` if no prompt is active.


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
  "prompt_type": "permission",
  "tool": "Bash",
  "input": "{\"command\":\"ls\"}",
  "questions": [],
  "question_current": 0
}
```

| Field | Type | Description |
|-------|------|-------------|
| `prompt_type` | string | `"permission"`, `"plan"`, `"question"` |
| `tool` | string or null | Tool name (permission prompts) |
| `input` | string or null | Truncated tool input (permission prompts) |
| `auth_url` | string or null | OAuth authorization URL (setup oauth_login prompts) |
| `questions` | QuestionContext[] | All questions in a multi-question dialog |
| `question_current` | int | 0-indexed current question; equals `questions.len()` at confirm phase |


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
  "option": 1,
  "text": null
}
```

| Field | Type | Description |
|-------|------|-------------|
| `option` | int or null | 1-indexed option number |
| `text` | string or null | Freeform text (used when selecting "Other") |
