# Claude Code Support

Coop provides first-class support for Claude Code via the `--agent claude` flag.
The Claude driver activates three structured detection tiers, hook-based event
ingestion, prompt response encoding, startup prompt handling, and session resume.

```
coop --agent claude --port 8080 -- claude --dangerously-skip-permissions
```


## Detection Tiers

When `--agent claude` is set, coop activates up to four detection tiers. Lower
tier numbers are higher confidence; the composite detector always prefers the
most-confident source.

| Tier | Source | Confidence | How it works |
|------|--------|------------|--------------|
| 1 | Hook events | Highest | Named pipe receives push events from Claude's hook system |
| 2 | Session log | High | File watcher tails `~/.claude/projects/<hash>/*.jsonl` |
| 3 | Stdout JSONL | Medium | Parses JSONL when Claude runs with `--print --output-format stream-json` |
| 4 | Process monitor | Low | Universal fallback: process alive, PTY activity, exit status |

Tier 5 (screen parsing) is **not** used for Claude since the structured tiers
provide reliable state classification.

### Tier 1: Hook Events

Coop creates a named FIFO pipe before spawning Claude and writes a hook
configuration file to `~/.claude/config/coop-hooks.json`. Claude loads this
via `--hook-config`.

Two hooks are registered:

- **PostToolUse** -- fires after each tool call, writes the tool name
- **Stop** -- fires when the agent stops

The hooks execute shell commands that write JSON to `$COOP_HOOK_PIPE`:

```json
{
  "hooks": {
    "PostToolUse": [{
      "type": "command",
      "command": "echo '{\"event\":\"post_tool_use\",\"tool\":\"'\"$TOOL_NAME\"'\"}' > \"$COOP_HOOK_PIPE\""
    }],
    "Stop": [{
      "type": "command",
      "command": "echo '{\"event\":\"stop\"}' > \"$COOP_HOOK_PIPE\""
    }]
  }
}
```

State mapping:

| Hook event | Agent state |
|------------|-------------|
| `AgentStop` / `SessionEnd` | `WaitingForInput` |
| `ToolComplete` | `Working` |

### Tier 2: Session Log Watching

Coop watches Claude's session log file for new JSONL entries. Each line is
parsed by `parse_claude_state()` to classify the agent's state.

Log discovery order:

1. `CLAUDE_CONFIG_DIR` environment variable
2. Default: `~/.claude/projects/<workspace-hash>/`
3. Watch for a new `.jsonl` file after spawn
4. Or: pass `--session-id <uuid>` to Claude for a known log path

When resuming a session, the log watcher starts from the byte offset where the
previous session left off, avoiding re-processing old entries.

### Tier 3: Structured Stdout

When Claude runs with `--print --output-format stream-json`, its stdout is a
JSONL stream. Coop feeds the raw PTY bytes through a JSONL parser and
classifies each entry with the same `parse_claude_state()` function. This tier
requires both flags to be present.

### Tier 4: Process Monitor

Universal fallback with no Claude-specific knowledge. Detects whether the
process is alive, whether the PTY has recent activity, and reports the exit
status. Provides coarse working-vs-idle detection.


## State Classification

Claude session log entries are classified into `AgentState` values:

```
parse_claude_state(json) ->
  error field present          => Error { detail }
  non-assistant message type   => Working
  assistant message with:
    tool_use "AskUserQuestion" => AskUser { prompt }
    other tool_use             => Working
    thinking block             => Working
    text-only content          => WaitingForInput
    empty content              => WaitingForInput
```

The full set of agent states:

| State | Meaning |
|-------|---------|
| `Starting` | Initial state before first detection |
| `Working` | Agent is executing (tool calls, thinking) |
| `WaitingForInput` | Agent is idle, ready for a nudge |
| `PermissionPrompt` | Agent is requesting tool permission |
| `PlanPrompt` | Agent is presenting a plan for approval |
| `AskUser` | Agent invoked `AskUserQuestion` |
| `Error` | An error occurred (e.g. rate limit) |
| `AltScreen` | Terminal switched to alternate screen |
| `Exited` | Child process exited |


## Prompt Context

When the agent enters a prompt state (`PermissionPrompt`, `PlanPrompt`,
`AskUser`), coop extracts structured context from the session log or screen.

**Permission prompts** -- extracted from the last `tool_use` block:
- `tool`: tool name (e.g. `"Bash"`, `"Edit"`)
- `input_preview`: truncated JSON of the tool input (~200 chars)

**AskUser questions** -- extracted from the `AskUserQuestion` tool input:
- `question`: the question text
- `options`: array of option labels

**Plan prompts** -- extracted from the terminal screen:
- `screen_lines`: raw lines from the rendered screen


## Encoding

Coop encodes nudge messages and prompt responses as PTY byte sequences written
to Claude's terminal input.

### Nudge

Sends a plain-text message followed by carriage return:

```
{message}\r
```

Only succeeds when the agent is in `WaitingForInput`.

### Prompt Responses

| Prompt type | Action | Bytes |
|-------------|--------|-------|
| Permission | Accept | `y\r` |
| Permission | Deny | `n\r` |
| Plan | Accept | `y\r` |
| Plan | Reject | `n\r` + 100ms delay + `{feedback}\r` |
| AskUser | Option N (1-indexed) | `{n}\r` |
| AskUser | Freeform text | `{text}\r` |


## Startup Prompts

Claude may present blocking prompts during startup before reaching the idle
state. When `--skip-startup-prompts` is enabled (default for `--agent claude`),
coop detects and auto-responds to these:

| Prompt | Detection pattern | Auto-response |
|--------|-------------------|---------------|
| Workspace trust | "Do you trust the files in this folder?" | `y\r` |
| Permission bypass | "Allow tool use without prompting?" | `y\r` |
| Login required | "Please sign in" | None (cannot auto-handle) |

Detection scans the last 5 non-empty lines of the rendered screen for known
keyword patterns.


## Session Resume

When coop restarts, it can reconnect to a previous Claude session. The
`--resume` flag triggers session discovery:

1. **Discover** the most recent `.jsonl` log in `~/.claude/projects/<workspace-hash>/`
2. **Parse** the log to recover the last agent state, byte offset, and conversation ID
3. **Append** `--continue --session-id <id>` to Claude's command-line arguments
4. **Start** the log watcher from the recovered byte offset

This allows coop to resume monitoring an existing Claude session without
re-processing the full log history.


## Idle Grace Timer

The grace timer prevents false idle transitions during rapid tool execution.
When a lower-confidence tier reports idle:

1. **Trigger**: record the current session log byte offset
2. **Wait**: default 60 seconds (configurable via `--idle-grace`)
3. **Confirm**: verify the log hasn't grown and state is still idle
4. **Emit**: if confirmed, emit `WaitingForInput` to consumers
5. **Cancel**: if activity detected, discard the idle signal

States from equal-or-higher confidence tiers bypass the grace timer entirely.
Terminal states (`Exited`) are always accepted immediately.


## Environment Variables

Coop sets the following environment variables on the Claude child process:

| Variable | Purpose |
|----------|---------|
| `COOP=1` | Marker that the process is running under coop |
| `COOP_HOOK_PIPE` | Path to the named FIFO for hook events |
| `COOP_SOCKET` | Optional socket path for sidecar communication |
| `TERM=xterm-256color` | Terminal type for the child PTY |


## CLI Flags

Flags relevant to Claude sessions:

| Flag | Default | Description |
|------|---------|-------------|
| `--agent claude` | -- | Enable Claude-specific detection and encoding |
| `--idle-grace SECS` | `60` | Grace timer duration before confirming idle |
| `--skip-startup-prompts` | `true` (for Claude) | Auto-handle workspace trust and permission prompts |
| `--resume HINT` | -- | Discover and resume a previous session |
