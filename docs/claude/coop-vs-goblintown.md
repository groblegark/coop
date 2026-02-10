# Coop vs Goblintown

Coop and Goblintown (Gas Town) both manage Claude Code sessions but at
different layers and with fundamentally different philosophies. Coop provides
the **session layer** (spawn, monitor, encode input). Goblintown provides the
**orchestration layer** (polecats, witness supervision, bead tracking, merge
queue, multi-agent coordination).

This document compares the two to clarify scope boundaries and identify where
coop replaces, overlaps with, or complements Goblintown's tmux-based session
management.


## Architecture

Goblintown currently manages Claude sessions directly through tmux:

```
Witness/Deacon → SessionManager → Tmux → tmux session → Claude Code
                      ↓                       ↓
               config beads → settings    pane-died hook
               gt prime (SessionStart)    health check pings
```

With coop as the session backend, the stack becomes:

```
Witness/Deacon → CoopAdapter → HTTP/gRPC → coop → PTY → Claude Code
                                             ↓
                                      multi-tier detection
```

Coop replaces the `Tmux` wrapper + `tmux capture-pane` + health check pings
with a single process that owns the PTY, renders the terminal, classifies
agent state, and exposes it over an API.


## Key Conceptual Differences

Goblintown and coop/oddjobs have fundamentally different detection models:

| Aspect | Goblintown | Coop |
|--------|------------|------|
| **State model** | Agent self-reports via `gt done`, `gt help`, protocol messages | Coop classifies state from hooks, logs, screen, process liveness |
| **Idle detection** | Health check pings (nudge + wait for response) | Multi-tier detection + tier-priority resolution |
| **Prompt handling** | Not handled at session layer; agents auto-approve via `--dangerously-skip-permissions` | Detected via hooks/screen → reported as `Prompt` state → consumer responds |
| **Work assignment** | Hook-driven: `gt prime` injects context at SessionStart | Consumer sends nudge message via API |
| **Completion signal** | Agent calls `gt done` → witness processes POLECAT_DONE | Coop detects `WaitingForInput` or `Exited` state |
| **Stuck recovery** | Deacon health checks: 3 failures → force-kill → respawn | Consumer monitors state events, decides recovery strategy |

Goblintown agents are **autonomous workers** that self-report milestones.
Coop provides **passive observation** with structured state classification.


## Feature Comparison

### Session management

| Capability | Goblintown (tmux) | Coop |
|------------|-------------------|------|
| Spawn | `tmux new-session` + `respawn-pane` (race-free) | Native PTY via `forkpty` + `exec` |
| Terminal rendering | `tmux capture-pane -p` (raw text) | VTE parser (`avt` crate), rendered screen |
| Input injection | `tmux send-keys -l` with per-session mutex | PTY write (bytes to master fd) |
| Kill | `tmux kill-session` | SIGHUP → 10s wait → SIGKILL |
| Liveness check | `pane_dead` flag + `pane_dead_status` | Tier 4 process monitor |
| Exit code | `tmux display-message "#{pane_dead_status}"` | Child waitpid |
| Session naming | `gt-{rig}-{polecat}` | N/A (coop owns one child) |
| Crash preservation | `remain-on-exit` + `CaptureDeadPaneOutput()` | VTE ring buffer preserves output |
| Input serialization | `sessionNudgeLocks` (Go `sync.Map` per session) | Single-writer lock (HTTP/WS acquire/release) |

### State detection

| Mechanism | Goblintown | Coop |
|-----------|------------|------|
| Agent self-reporting | Primary: `gt done`, `gt help`, protocol messages via mail | N/A (passive observation) |
| Notification hook | Not used for detection | Tier 1: `idle_prompt`/`permission_prompt` → FIFO |
| PreToolUse hook | Used for workflow guards (`gt tap guard pr-workflow`) | Tier 1: `AskUserQuestion`/`ExitPlanMode`/`EnterPlanMode` → FIFO |
| PostToolUse hook | Used for nudge/inject drain (`gt inject drain`) | Tier 1: tool name → FIFO → Working signal |
| Stop hook | Used for decision checking (`gt decision turn-check`) | Tier 1: FIFO + HTTP gating |
| SessionStart hook | Primary: runs `gt prime --hook && gt mail check --inject` | Not used |
| UserPromptSubmit hook | Runs `gt mail check --inject && gt decision auto-close` | Not used |
| Session log watcher | Not used | Tier 2 (file watcher, incremental JSONL) |
| Stdout JSONL | Not used | Tier 3 (`--print --output-format stream-json`) |
| Process monitor | `pane_dead` flag checks | Tier 4 (process alive + PTY activity) |
| Screen parsing | Not used | Tier 5 (setup dialogs, workspace trust, idle prompt) |
| Health check pings | Deacon sends HEALTH_CHECK nudge, waits for response | N/A (consumer's responsibility) |

Goblintown uses hooks for **workflow orchestration** (context injection, mail
delivery, decision management). Coop uses hooks for **state detection** (what
is the agent doing right now?). The hook types overlap but serve completely
different purposes.

### Idle / stuck detection

| Aspect | Goblintown | Coop |
|--------|------------|------|
| Mechanism | Deacon health check pings via tmux nudge | Multi-tier state detection + idle timeout |
| Ping timeout | 30s default (configurable via role bead) | N/A |
| Failure threshold | 3 consecutive failures → force-kill | N/A (consumer decides) |
| Cooldown | 5 minutes between force-kills | N/A |
| Grace timer | N/A | N/A (no grace timer; composite detector uses tier-priority resolution) |
| Cancellation | Agent responds to health check | Any activity (log growth, tool call) |
| Recovery | Force-kill session → witness respawns polecat | Consumer nudges or kills via API |

Goblintown's deacon actively probes agents. Coop passively observes and
reports state; the consumer decides whether and how to recover.

### Startup prompts

| Prompt | Goblintown handling | Coop handling |
|--------|---------------------|---------------|
| Bypass permissions | `AcceptBypassPermissionsWarning()` via tmux capture-pane polling | Suppressed from idle detection; orchestrator responds via API |
| Workspace trust (text) | Not explicitly handled (agents run with `--dangerously-skip-permissions`) | Suppressed from idle detection; orchestrator responds via API |
| Workspace trust (dialog) | Not explicitly handled | Reported as `Prompt(Permission, subtype="trust")` |
| Login/onboarding | Not handled (expects pre-authenticated credentials) | Reported as `Prompt(Setup)` |

Goblintown sidesteps most startup prompts by running with
`--dangerously-skip-permissions` and pre-authenticating credentials. The only
prompt it explicitly handles is the bypass permissions warning dialog. Coop
detects text-based startup prompts to suppress false idle signals but does not
auto-respond; the orchestrator must handle them via the API.

### Startup sequence

| Step | Goblintown | Coop |
|------|------------|------|
| 1. Materialize settings | Config beads → merge → write `settings.json` + `.mcp.json` | Write `coop-settings.json` (hooks only) |
| 2. Create session | `tmux new-session` + `respawn-pane` with command | `forkpty` + `exec` with `--settings` flag |
| 3. Set environment | `tmux set-environment` per variable | Child process inherits env from coop |
| 4. Wait for ready | `WaitForCommand()` polls for Claude process | Tier 5 screen detector polls for `❯` prompt |
| 5. Accept prompts | `AcceptBypassPermissionsWarning()` | Auto-respond or report as `Prompt(Setup)` |
| 6. Inject context | SessionStart hook runs `gt prime --hook && gt mail check --inject` | N/A (consumer sends nudge) |
| 7. Send work | Beacon + startup nudge (with fallback matrix) | N/A (consumer sends nudge) |

Goblintown's startup is a multi-step orchestration sequence with fallback
paths for agents with/without hooks. Coop's startup is simpler: spawn the
process and start detecting state.

### Hook usage

Goblintown and coop both configure Claude hooks but for entirely different
purposes. The hook types overlap, so coexistence requires careful matcher
coordination.

| Hook | Goblintown purpose | Coop purpose |
|------|-------------------|--------------|
| **SessionStart** | Context injection: `gt prime`, mail check, deacon notification | Not used |
| **UserPromptSubmit** | Mail check, decision auto-close, inject | Not used |
| **PreToolUse** | Workflow guards: `gt tap guard pr-workflow` on git/gh commands | State detection: AskUserQuestion, ExitPlanMode, EnterPlanMode |
| **PostToolUse** | Drain injected messages: `gt inject drain`, `gt nudge drain` | State detection: tool completion → Working signal |
| **PreCompact** | Context refresh: `gt prime --hook` | Not used |
| **Stop** | Decision checking: `gt decision turn-check` | Detection (FIFO) + gating (HTTP verdict) |

**Coexistence strategy**: Settings files are separate (`settings.json` for
Goblintown, `coop-settings.json` for coop). Claude merges settings from
multiple files. PreToolUse matchers don't conflict (Goblintown matches
`Bash(gh pr create*)` etc.; coop matches `ExitPlanMode|AskUserQuestion|EnterPlanMode`).

### Prompt handling

| Prompt type | Goblintown | Coop |
|-------------|------------|------|
| Permission | Not handled (agents use `--dangerously-skip-permissions`) | Detected via Notification hook (T1) + screen (T5) → `Prompt(Permission)` |
| AskUser | Not handled (agents are autonomous) | Detected via PreToolUse hook (T1) + log (T2) → `Prompt(Question)` |
| Plan | Not handled (agents are autonomous) | Detected via PreToolUse hook (T1) → `Prompt(Plan)` |
| Decision | Custom system: `gt decision` creates human-in-the-loop records | N/A (consumer-level) |

Goblintown agents are fully autonomous (`--dangerously-skip-permissions`) and
don't encounter permission prompts during normal operation. Coop is designed
for scenarios where prompts need consumer approval.

### Input encoding

| Action | Goblintown | Coop |
|--------|------------|------|
| Nudge | `tmux send-keys -l` + Enter + SIGWINCH (serialized per-session) | `{message}\r` via PTY write |
| Beacon | Printed to CLI prompt or sent as nudge (fallback) | N/A (consumer responsibility) |
| Work instructions | Nudge via tmux (with optional delay for non-hook agents) | Consumer sends nudge via API |
| Permission respond | N/A (auto-approved) | `{n}\r` (numbered option) |
| AskUser respond | N/A | `{n}\r` or `{n}` (multi-question) |
| Plan respond | N/A | `{n}\r` (numbered option) |
| Input clearing | Not done (serialized writes prevent interleaving) | N/A (consumer's responsibility) |
| Input debouncing | 200ms+ between send-keys calls | N/A (consumer's responsibility) |

### Session resume

| Aspect | Goblintown | Coop |
|--------|------------|------|
| Mechanism | Beacon injection into `/resume` picker | `--resume HINT` → discovers log → `claude --resume <id>` |
| Session ID | Not tracked explicitly | Extracted from session log `sessionId` field |
| Predecessor discovery | Beacon format: `[GAS TOWN] recipient <- sender • timestamp • topic` | Log file discovery by workspace hash |
| Context recovery | `gt prime` re-injects context on SessionStart | Log watcher resumes from byte offset |
| When used | Every polecat spawn (beacon always present) | Coop process restart |

Goblintown uses beacons for predecessor discovery (Claude's `/resume` picker
shows recent sessions with beacon text). Coop's resume is for reconnecting
to a previous conversation when the coop process itself restarts.

### Work completion

| Aspect | Goblintown | Coop |
|--------|------------|------|
| Signal | Agent calls `gt done` → witness processes POLECAT_DONE | Coop detects `WaitingForInput` or `Exited` |
| Lifecycle | Polecat ceases to exist after `gt done` | Session stays until consumer shuts down |
| Merge | Submitted to refinery merge queue → sequential rebase | N/A (consumer-level) |
| Conflict handling | Rebase-as-work: fresh polecat re-implements | N/A (consumer-level) |

### Settings materialization

| Aspect | Goblintown | Coop |
|--------|------------|------|
| Source | Config beads (structured metadata in beads DB) | Hard-coded hook config in Rust |
| Merge strategy | Append hooks, override top-level keys, null suppresses | Single settings file, no merging |
| Role differentiation | Autonomous vs interactive templates | One config for all sessions |
| MCP config | Materialized from beads to `.mcp.json` | Not managed |
| Per-agent isolation | Settings in `polecats/<name>/` home directory | Settings in session dir |
| Fallback | Embedded templates if beads unavailable | N/A (always generates) |

### Credential management

| Aspect | Goblintown | Coop |
|--------|------------|------|
| Auth method | Per-account OAuth tokens or API keys | Not managed (consumer's responsibility) |
| Multi-account | Supported: `CLAUDE_CONFIG_DIR` per account | Not managed |
| Injection | `ANTHROPIC_AUTH_TOKEN` env var on tmux session | Consumer sets env before coop spawn |
| Rate limit detection | Pre-spawn check via `ratelimit.Tracker` | Not managed |


## What coop replaces

| Goblintown component | Replaced by |
|----------------------|-------------|
| `Tmux` wrapper (spawn, send, kill, capture) | Coop PTY + VTE + HTTP API |
| `tmux capture-pane` for screen content | `GET /api/v1/screen` |
| `tmux send-keys` for input / nudges | `POST /api/v1/input` or `POST /api/v1/agent/nudge` |
| `pane_dead` liveness checks | Coop Tier 4 process monitor + `Exited` state |
| `AcceptBypassPermissionsWarning()` | Coop detects and suppresses from idle; orchestrator responds |
| `WaitForCommand()` polling for Claude process | Coop Tier 5 screen detector (polls for `❯`) |
| `sessionNudgeLocks` (per-session write mutex) | Coop single-writer lock |
| Health check ping/response cycle (for detection) | Coop multi-tier state detection |

## What coop does not replace

| Goblintown component | Why |
|----------------------|-----|
| Config bead materialization | Orchestrator-level: merges settings from structured metadata layers |
| MCP configuration | Orchestrator-level: server config from beads |
| SessionStart hook (`gt prime`, mail inject) | Orchestrator-level: context injection at session start |
| UserPromptSubmit hook (mail, decisions) | Orchestrator-level: per-turn workflow |
| PreToolUse guards (`gt tap guard`) | Orchestrator-level: workflow policy enforcement |
| Witness protocol (POLECAT_DONE, HELP, etc.) | Orchestrator-level: agent milestone tracking |
| Deacon health check policy | Orchestrator-level: stuck recovery thresholds and cooldowns |
| Beacon / predecessor discovery | Orchestrator-level: session continuity for `/resume` picker |
| Credential management | Orchestrator-level: multi-account OAuth, rate limit tracking |
| Merge queue (refinery) | Orchestrator-level: sequential rebase, conflict handling |
| Polecat lifecycle (spawn, work, done, die) | Orchestrator-level: ephemeral worker management |
| Work assignment (`gt sling`, beads) | Orchestrator-level: issue tracking and dispatch |
| Inter-agent mail | Orchestrator-level: messaging between agents |


## Migration Path

### Phase 1: Coop alongside tmux (opt-in)

- Ship coop binary alongside Goblintown
- Add `CoopBackend` implementing the same interface as `Tmux` wrapper
- Opt-in via flag: `--session-backend=coop`
- Existing tmux path unchanged
- Validate: screen capture, nudge delivery, process lifecycle

### Phase 2: Coop for new polecats (default)

- Default new polecat sessions to coop
- Goblintown spawns `coop --agent claude -- claude ...` instead of tmux
- Subscribe to coop state events for health monitoring (replaces ping cycle)
- Keep tmux path as fallback

### Phase 3: Remove tmux dependency

- Delete `internal/tmux/` package
- All sessions through coop
- Only after weeks of production validation

### Hook coexistence

During migration, both Goblintown and coop hooks must coexist:

- Goblintown writes `settings.json` (workflow hooks)
- Coop writes `coop-settings.json` (detection hooks)
- Claude merges both via `--settings` flag
- PreToolUse matchers are disjoint (no conflicts)
- PostToolUse: both use wildcard matcher `""` — coop writes to FIFO (fast, no side effects), Goblintown runs `gt inject drain` (idempotent)
- Stop: both use wildcard — coop writes to FIFO + curls gating endpoint, Goblintown runs `gt decision turn-check` — order doesn't matter since both are non-blocking
