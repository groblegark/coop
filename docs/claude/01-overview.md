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

Coop creates a named FIFO pipe before spawning Claude and writes a settings
file containing the hook configuration. Claude loads this via `--settings`.

Two hooks are registered:

- **PostToolUse** -- fires after each tool call, writes the tool name
- **Stop** -- fires when the agent stops

The hooks execute shell commands that write JSON to `$COOP_HOOK_PIPE`:

```json
{
  "hooks": {
    "PostToolUse": [{
      "matcher": "",
      "hooks": [{
        "type": "command",
        "command": "echo '{\"event\":\"post_tool_use\",\"tool\":\"'\"$TOOL_NAME\"'\"}' > \"$COOP_HOOK_PIPE\""
      }]
    }],
    "Stop": [{
      "matcher": "",
      "hooks": [{
        "type": "command",
        "command": "echo '{\"event\":\"stop\"}' > \"$COOP_HOOK_PIPE\""
      }]
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


### Composite Detector

The `CompositeDetector` runs all active tiers concurrently and resolves
conflicts with these rules:

- **Same or higher confidence tier**: state accepted immediately
- **Lower confidence tier, non-idle**: accepted immediately
- **Lower confidence tier, idle**: routed through the grace timer
- **Terminal state (`Exited`)**: accepted immediately from any tier, cancels grace timer
- **Duplicate state**: suppressed (updates tier tracking only)


## State Classification

Claude session log entries (Tiers 2 and 3) are classified into `AgentState`
values by `parse_claude_state()`:

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

| State | Wire name | Meaning |
|-------|-----------|---------|
| `Starting` | `starting` | Initial state before first detection |
| `Working` | `working` | Executing tool calls or thinking |
| `WaitingForInput` | `waiting_for_input` | Idle, ready for a nudge |
| `PermissionPrompt` | `permission_prompt` | Requesting tool permission |
| `PlanPrompt` | `plan_prompt` | Presenting a plan for approval |
| `AskUser` | `ask_user` | Invoked `AskUserQuestion` tool |
| `Error` | `error` | Error occurred (rate limit, auth, etc.) |
| `AltScreen` | `alt_screen` | Terminal switched to alternate screen |
| `Exited` | `exited` | Child process exited |


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
state. Coop reports these as permission prompts through the normal state
detection pipeline. The orchestrator (consumer) is responsible for responding
via the API, just like any other permission prompt.

| Prompt | Detection pattern |
|--------|-------------------|
| Workspace trust | "Do you trust the files in this folder?" |
| Permission bypass | "Allow tool use without prompting?" |
| Login required | "Please sign in" |


## Session Resume

When coop restarts, it can reconnect to a previous Claude conversation. The
`--resume` flag triggers session discovery:

1. **Discover** the most recent `.jsonl` log in `~/.claude/projects/<workspace-hash>/`
2. **Parse** the log to recover the last agent state, byte offset, and conversation ID
3. **Append** `--resume <id>` to Claude's command-line arguments (or `--continue` if no ID)
4. **Append** `--settings <path>` so hooks are active in the new process
5. **Start** the log watcher from the recovered byte offset

This spawns a new Claude process that loads the previous conversation history,
then resumes log watching from where the previous coop session left off. The
conversation ID is extracted from the `sessionId` or `conversationId` field in
the log's first entry.


## Idle Grace Timer

Between tool calls, Claude briefly enters a text-only state that looks like
`WaitingForInput` before starting the next tool. The grace timer prevents
these false idle transitions.

When a lower-confidence tier reports `WaitingForInput`:

1. **Trigger**: record the current session log byte offset
2. **Wait**: default 60 seconds (configurable via `--idle-grace`)
3. **Confirm**: verify the log hasn't grown and state is still idle
4. **Emit**: if confirmed, emit `WaitingForInput` to consumers
5. **Cancel**: if activity detected, discard the idle signal

States from equal-or-higher confidence tiers bypass the grace timer entirely.
Terminal states (`Exited`) are always accepted immediately.

Activity that cancels the timer:

| Activity | Why it cancels |
|----------|----------------|
| Tool calls | `tool_use` block appears in log |
| Extended thinking | `thinking` block appears in log |
| Subagent execution | `tool_use` for Task persists throughout |
| Long Bash commands | `tool_use` block persists until result |
| Streaming text | Log grows as response is written |


## Environment Variables

Coop sets the following environment variables on the Claude child process:

| Variable | Purpose |
|----------|---------|
| `COOP=1` | Marker that the process is running under coop |
| `COOP_HOOK_PIPE` | Path to the named FIFO for hook events |
| `TERM=xterm-256color` | Terminal type for the child PTY |


## CLI Flags

Flags relevant to Claude sessions:

| Flag | Default | Description |
|------|---------|-------------|
| `--agent claude` | -- | Enable Claude-specific detection and encoding |
| `--idle-grace SECS` | `60` | Grace timer duration before confirming idle |
| `--resume HINT` | -- | Discover and resume a previous session |


## Source Layout

```
crates/cli/src/driver/claude/
├── mod.rs         # ClaudeDriver: wires up detectors and encoders
├── detect.rs      # HookDetector (T1), LogDetector (T2), StdoutDetector (T3)
├── state.rs       # parse_claude_state() — JSONL → AgentState
├── hooks.rs       # Hook config generation, environment setup
├── setup.rs       # Pre-spawn session preparation (FIFO, settings, args)
├── prompt.rs      # PromptContext extraction (permission, question, plan)
├── encoding.rs    # ClaudeNudgeEncoder, ClaudeRespondEncoder
├── startup.rs     # Startup prompt detection patterns
└── resume.rs      # Session log discovery, state recovery, --resume args
```
