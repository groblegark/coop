# Coop vs Oddjobs Claude Adapter

Coop and oddjobs both manage Claude Code sessions but at different layers.
Coop provides the **session layer** (spawn, monitor, encode input). Oddjobs
provides the **orchestration layer** (jobs, decisions, stuck recovery policy).

This document compares the two to clarify scope boundaries and identify where
coop replaces, overlaps with, or complements oddjobs' Claude adapter.


## Architecture

Oddjobs currently manages Claude sessions directly through tmux:

```
Engine → ClaudeAgentAdapter → TmuxAdapter → tmux → Claude Code
                ↓
         Watcher (file)  →  session log JSONL
```

With coop as the session backend, the stack becomes:

```
Engine → CoopAdapter → HTTP/gRPC → coop → PTY → Claude Code
                                     ↓
                              multi-tier detection
```

Coop replaces `TmuxAdapter` + the background `Watcher` task with a single
process that owns the PTY, renders the terminal, and classifies agent state.


## Feature Comparison

### Session management

| Capability | Oddjobs (tmux) | Coop |
|------------|----------------|------|
| Spawn | `tmux new-session -d -s oj-... -c {cwd}` | Native PTY via `forkpty` + `exec` |
| Terminal rendering | `tmux capture-pane -p` (raw text) | VTE parser (`avt` crate), rendered screen |
| Input injection | `tmux send-keys` / `send-keys -l` | PTY write (bytes to master fd) |
| Kill | `tmux kill-session` | SIGHUP → 10s wait → SIGKILL |
| Liveness check | `tmux has-session` + `ps`/`pgrep` | Tier 4 process monitor |
| Exit code | `tmux display-message "#{pane_dead_status}"` | Child waitpid |
| Session naming | `oj-{job}-{agent}-{random}` | N/A (coop owns one child) |
| `remain-on-exit` | Set after spawn to preserve output | N/A (VTE ring buffer preserves output) |

### State detection

| Mechanism | Oddjobs | Coop |
|-----------|---------|------|
| Notification hook (`idle_prompt`, `permission_prompt`) | Primary (instant via `oj agent hook notify`) | Not used |
| PreToolUse hook (`AskUserQuestion`, `ExitPlanMode`) | Primary (instant via `oj agent hook pretooluse`) | Not used |
| PostToolUse hook | Not used | Tier 1 (tool name to FIFO pipe) |
| Stop hook event | Not used for detection (used to gate exit) | Tier 1 (stop event to FIFO pipe) |
| Session log watcher | Fallback (5s poll via file notifications) | Tier 2 (file watcher, incremental) |
| Stdout JSONL | Not supported | Tier 3 (`--print --output-format stream-json`) |
| Process monitor | `ps`/`pgrep` in watcher loop | Tier 4 (universal fallback) |
| Screen parsing | Not used for Claude | Not used for Claude (Tier 5, `unknown` only) |

Oddjobs relies on Claude's built-in Notification and PreToolUse hooks for
instant state detection, with the session log watcher as a polling fallback.

Coop uses its own PostToolUse and Stop hooks (written to a FIFO pipe) as
Tier 1, the session log as Tier 2, and combines them through a composite
detector with tier-priority resolution.

### Idle detection

| Aspect | Oddjobs | Coop |
|--------|---------|------|
| Grace timer | 60s default (`OJ_IDLE_GRACE_MS`) | 60s default (`--idle-grace`) |
| Confirmation | Log file size unchanged + state still WaitingForInput | Log byte offset unchanged + state still idle |
| Cancellation | Agent transitions to Working → immediate cancel | Any activity (log growth) → cancel |
| Self-trigger prevention | Suppresses auto-resume for 60s after nudge | N/A (consumer's responsibility) |

Both systems use the same two-phase idle confirmation. The difference is that
oddjobs embeds the policy (nudge attempts, escalation) while coop only reports
the confirmed idle state.

### Startup prompts

| Prompt | Oddjobs detection | Coop detection |
|--------|-------------------|----------------|
| Bypass permissions | `capture-pane` polling: "Bypass Permissions mode" | Screen scan: "Allow tool use without prompting?" |
| Workspace trust | `capture-pane` polling: "Accessing workspace" + "1. Yes" | Screen scan: "Do you trust the files" |
| Workspace trust (late) | Watcher checks pane output during log wait | Same screen scan mechanism |
| Login/onboarding | `capture-pane` polling: "Select login method" | Screen scan: "Please sign in" |

| Prompt | Oddjobs response | Coop response |
|--------|------------------|---------------|
| Bypass permissions | Sends `"2"` (accept numbered option) | Reported as permission prompt; orchestrator responds via API |
| Workspace trust | Sends `"1"` (trust numbered option) | Reported as permission prompt; orchestrator responds via API |
| Login/onboarding | Kills session, returns `SpawnFailed` | Reported as permission prompt; orchestrator decides |

Coop does not auto-handle startup prompts. It reports them as permission
prompts through the state detection pipeline. The orchestrator is responsible
for responding.

### Prompt handling

| Prompt type | Oddjobs | Coop |
|-------------|---------|------|
| Permission | Detected via Notification hook → decision created → human resolves → `y`/`n` sent | Detected via log/hooks → `PermissionPrompt` state + context → consumer responds via API |
| AskUser | Detected via PreToolUse hook → decision created → human picks option → number sent | Detected via log → `AskUser` state + context → consumer responds via API |
| Plan | Detected via PreToolUse hook → decision created → human accepts/revises | Detected via screen → `PlanPrompt` state + context → consumer responds via API |

Oddjobs creates **decisions** (human-in-the-loop records with numbered options,
context messages, and resolution actions). Coop emits **state changes** with
structured prompt context. The consumer (orchestrator) decides what to do.

### Session resume

| Aspect | Oddjobs | Coop |
|--------|---------|------|
| Mechanism | `claude --resume {session_id}` (new tmux session, loads old conversation) | `--resume HINT` → discovers log → `claude --resume <id>` (or `--continue` fallback) |
| Session ID tracking | Stored in `Job.session_id` via WAL | Extracted from session log `sessionId` field |
| Log offset | Not tracked (watcher starts fresh) | Recovered from log file size |
| Daemon restart | `adapter.reconnect()` re-attaches watcher to existing tmux session | `--resume` discovers log and reconnects |
| When used | Job resume event, agent step restart, error recovery | Coop process restart |

Oddjobs' `reconnect()` is for daemon restart: the tmux session is still alive,
so the adapter just re-attaches the file watcher. Coop's `--resume` spawns a
new Claude process that continues the old conversation.

### Session suspend

Oddjobs has job-level suspension (`StepStatus::Suspended`):
- The tmux session stays alive
- The engine stops processing state changes for the job
- Suspended workspaces are protected from pruning
- `JobResume` event re-activates monitoring

Coop has no explicit suspend. Consumers achieve the same effect by ignoring
state change events from the coop API.

### Input encoding

| Action | Oddjobs | Coop |
|--------|---------|------|
| Nudge | `session.send_literal(text)` + `session.send_enter()` with Esc clearing | `{message}\r` via PTY write |
| Permission accept | `session.send("y")` | `y\r` |
| Permission deny | `session.send("n")` | `n\r` |
| AskUser option | `session.send("{n}")` | `{n}\r` |
| Plan accept | Arrow key navigation + Enter | `y\r` |
| Plan reject | Arrow key navigation + Enter + feedback | `n\r` + 100ms delay + `{feedback}\r` |
| Input clearing | Esc → 50ms pause → Esc (clear any partial input) | N/A (consumer's responsibility) |

Oddjobs clears partial input before sending (Esc + pause + Esc) to handle
cases where the terminal has stale input. Coop writes directly; consumers
should ensure the agent is in the expected state before sending.


## Hooks

Oddjobs and coop use different Claude hooks for different purposes. Both sets
can coexist in the same hook configuration.

| Hook | Oddjobs | Coop |
|------|---------|------|
| **Stop** | Gates exit: blocks until agent signals completion via `oj emit agent:signal` | Detection: writes `{"event":"stop"}` to FIFO → Tier 1 idle signal |
| **Notification** | `idle_prompt`/`permission_prompt` → `oj agent hook notify` → instant state detection | Not used |
| **PreToolUse** | `AskUserQuestion`/`ExitPlanMode`/`EnterPlanMode` → `oj agent hook pretooluse` → decision creation | Not used |
| **PostToolUse** | Not used | Writes `{"event":"post_tool_use","tool":"..."}` to FIFO → Tier 1 working signal |
| **SessionStart** | Runs prime scripts (per-source) | Not used |

The Stop hook serves fundamentally different purposes: oddjobs uses it to
**prevent** Claude from exiting until the orchestrator is satisfied; coop uses
it to **detect** that Claude has stopped.


## What coop replaces

| Oddjobs component | Replaced by |
|--------------------|-------------|
| `TmuxAdapter` (spawn, send, kill, capture, configure) | Coop PTY + VTE + HTTP API |
| `Watcher` (session log file monitoring) | Coop Tier 2 log detector |
| Startup prompt polling via `capture_output()` | Coop screen-based startup prompt detection |
| `ps`/`pgrep` liveness checks | Coop Tier 4 process monitor |
| `tmux capture-pane` for screen content | `GET /api/v1/screen` |
| `tmux send-keys` for input | `POST /api/v1/input` |

## What coop does not replace

| Oddjobs component | Why |
|--------------------|-----|
| Decision system | Orchestrator-level: creates human-in-the-loop records, tracks resolution |
| Job lifecycle | Orchestrator-level: multi-step workflows, suspend/resume/cancel |
| Workspace management | Orchestrator-level: git worktrees, directory setup, cleanup |
| Stop hook (exit gate) | Orchestrator-level: requires agent signaling protocol (`oj emit agent:signal`) |
| PreToolUse hooks | Orchestrator-level: intercepts tools for decision creation |
| Notification hooks | Orchestrator-level: instant detection via CLI callback |
| Agent signaling | Orchestrator-level: `complete`/`escalate`/`continue` signals |
| Settings injection | Orchestrator-level: per-agent `claude-settings.json` with hooks and permissions |
| Stuck recovery policy | Orchestrator-level: nudge attempts, escalation thresholds, retry limits |
| Desktop notifications | Orchestrator-level: alert humans when decisions are needed |
