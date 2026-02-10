# Coop vs Goblintown

Coop provides the **session layer** (spawn, monitor, encode input).
Goblintown provides the **orchestration layer** (polecats, witness, beads,
merge queue, multi-agent coordination).

```
Before:  Witness/Deacon → SessionManager → Tmux → tmux → Claude Code
                               ↓                    ↓
                        config beads → settings   pane-died hook
                        gt prime (SessionStart)   health check pings

After:   Witness/Deacon → CoopAdapter → HTTP/gRPC → coop → PTY → Claude Code
                                                       ↓
                                                multi-tier detection
```

**Key difference**: Goblintown agents are autonomous workers that self-report
milestones (`gt done`, `gt help`). Coop provides passive observation with
structured state classification. These are complementary — Goblintown's
self-reporting protocol would run inside a coop-managed PTY.


## Session Management

| Capability           | GT | Coop | Notes                                              |
| -------------------- | -- | ---- | -------------------------------------------------- |
| Spawn                | ✓  | ✓    | `tmux new-session` + `respawn-pane` vs native PTY  |
| Terminal rendering   | ✓  | ✓+   | raw text vs VTE-parsed screen                      |
| Input injection      | ✓  | ✓    | `send-keys -l` with per-session mutex vs PTY write |
| Kill                 | ✓  | ✓    |                                                    |
| Liveness / exit code | ✓  | ✓    | `pane_dead` vs waitpid                             |
| Output preservation  | ✓  | ✓    | `remain-on-exit` vs ring buffer                    |
| Input serialization  | ✓  | ✓    | Go `sync.Map` per session vs single-writer lock    |


## State Detection

Goblintown uses hooks for **workflow orchestration** (context injection, mail, decisions).
Coop uses hooks for **state detection**.

The hook types overlap but serve completely different purposes.

| Signal                | GT | Coop | Notes                                                               |
| --------------------- | -- | ---- | ------------------------------------------------------------------- |
| Agent self-reporting  | ✓  | ✗    | `gt done`, `gt help`, protocol messages. Out of scope for coop      |
| Notification hook     | ✗  | ✓    | GT doesn't use for detection                                        |
| PreToolUse hook       | ✓  | ✓    | GT: workflow guards. Coop: prompt detection. Disjoint matchers      |
| PostToolUse hook      | ✓  | ✓    | GT: drain inject/nudge. Coop: Working signal. Both use `""` matcher |
| Stop hook             | ✓  | ✓    | GT: decision check. Coop: detection + gating. Both use `""` matcher |
| SessionStart hook     | ✓  | ✗    | GT only: `gt prime`, mail inject, deacon notify                     |
| UserPromptSubmit hook | ✓  | ✗    | GT only: mail check, decision auto-close                            |
| Session log watcher   | ✗  | ✓    | Tier 2                                                              |
| Stdout JSONL          | ✗  | ✓    | Tier 3                                                              |
| Process monitor       | ✓  | ✓    | Tier 4                                                              |
| Screen parsing        | ✗  | ✓    | Tier 5: setup dialogs, trust, idle prompt                           |
| Health check pings    | ✓  | ✗    | Deacon active probing. Replaced by passive state detection          |


## Prompt Handling

Goblintown agents run `--dangerously-skip-permissions` and don't encounter permission prompts during normal operation.
Coop supports that but is also designed to support scenarios where prompts need consumer approval.

| Prompt                    | GT | Coop |
| ------------------------- | -- | ---- |
| Permission detection      | ✗  | ✓    |
| Permission response       | ✗  | ✓    |
| AskUser detection         | ✗  | ✓    |
| AskUser response          | ✗  | ✓    |
| Plan detection            | ✗  | ✓    |
| Plan response             | ✗  | ✓    |
| Setup dialog detection    | ✗  | ✓    |
| Prompt context extraction | ✗  | ✓    |


## Startup Prompts

| Prompt             | GT | Coop | Notes                                                                         |
| ------------------ | -- | ---- | ----------------------------------------------------------------------------- |
| Bypass permissions | ✓  | ✓    | GT: auto-accepts via capture-pane. Coop: suppresses idle, reports to consumer |
| Workspace trust    | ✗  | ✓    | GT relies on `--dangerously-skip-permissions`                                 |
| Login/onboarding   | ✗  | ✓    | GT expects pre-authenticated credentials                                      |

Coop does not auto-respond to startup prompts.
It detects them (to suppress false idle signals) and reports them.
The orchestrator responds via the API.


## Idle / Stuck Detection

| Aspect                  | GT | Coop | Notes                                                     |
| ----------------------- | -- | ---- | --------------------------------------------------------- |
| Passive state detection | ✗  | ✓    | Multi-tier composite detector                             |
| Active health pings     | ✓  | ✗    | Deacon: 30s timeout, 3 failures → force-kill, 5m cooldown |

GT's deacon actively probes agents. Coop passively observes and reports state;
the consumer decides recovery strategy. GT's active probing is an
orchestrator-level policy that would consume coop's state events instead of
sending health check pings.


## Input Encoding

| Action             | GT | Coop | Notes                                                         |
| ------------------ | -- | ---- | ------------------------------------------------------------- |
| Nudge              | ✓  | ✓    | GT: `send-keys -l` + Enter + SIGWINCH. Coop: `{message}\r`    |
| Permission respond | ✗  | ✓    |                                                               |
| AskUser respond    | ✗  | ✓    |                                                               |
| Plan respond       | ✗  | ✓    |                                                               |
| Input debouncing   | ✓  | ✓    |                                                               |


## Session Resume

| Aspect                | GT | Coop |
| --------------------- | -- | ---- |
| Resume conversation   | ✓  | ✓    |
| Predecessor discovery | ✓  | ✗    |
| Log offset recovery   | ✗  | ✓    |

GT uses beacons (`[GAS TOWN] recipient <- sender • timestamp • topic`) for predecessor discovery in Claude's `/resume` picker.
Coop's `--resume` discovers the log file and passes `--resume <id>` to Claude.
These are complementary — GT's beacon injection would work inside a coop-managed session.


## Hooks Coexistence

Both GT and coop configure Claude hooks, but for different purposes. During
migration they coexist via separate settings files:

- GT writes `settings.json` (workflow hooks)
- Coop writes `coop-settings.json` (detection hooks)
- Claude merges both via `--settings`
- PreToolUse matchers are disjoint: GT matches `Bash(gh pr create*)` etc.; coop matches `ExitPlanMode|AskUserQuestion|EnterPlanMode`
- PostToolUse and Stop both use `""` matcher — GT commands are idempotent, coop's FIFO write is side-effect-free, order doesn't matter


## Out of Scope for Coop

These remain orchestrator-level concerns in Goblintown:

| Component | Description | Integration notes |
| --------- | ----------- | ----------------- |
| Config bead materialization | Merges settings from structured metadata layers | GT writes `settings.json`; coop writes `coop-settings.json` separately |
| MCP configuration | Server config materialized from beads to `.mcp.json` | Unchanged — coop doesn't touch MCP |
| SessionStart hook | `gt prime --hook && gt mail check --inject` | Runs inside coop PTY; coop doesn't need to know about it |
| UserPromptSubmit hook | Mail check, decision auto-close | Same |
| PreToolUse guards | `gt tap guard pr-workflow` on git/gh commands | Disjoint matchers from coop's hooks |
| Witness protocol | POLECAT_DONE, HELP, MERGED, RATE_LIMITED messages | Runs inside coop PTY via `gt` commands |
| Deacon health policy | Stuck recovery: thresholds, cooldowns, force-kill | Replaced by subscribing to coop state events |
| Beacon / predecessor | Session continuity for `/resume` picker | Beacon still printed to terminal inside coop PTY |
| Credential management | Multi-account OAuth, `CLAUDE_CONFIG_DIR`, rate limit tracking | Consumer sets env vars before spawning coop |
| Merge queue (refinery) | Sequential rebase, conflict → fresh polecat | Unchanged |
| Polecat lifecycle | Spawn, work, `gt done`, die | `gt done` runs inside coop PTY; coop detects the resulting state |
| Work assignment | `gt sling`, beads, hook-driven context injection | Unchanged |
| Inter-agent mail | Messaging between polecats, witness, deacon | Runs inside coop PTY via `gt mail` |


## Migration Path

1. **Phase 1 — opt-in**: Ship coop binary, add `CoopBackend` behind `--session-backend=coop` flag, tmux path unchanged
2. **Phase 2 — default**: New polecats use coop, subscribe to state events for health monitoring (replaces ping cycle), tmux as fallback
3. **Phase 3 — remove tmux**: Delete `internal/tmux/` (~1,800 lines), all sessions through coop
