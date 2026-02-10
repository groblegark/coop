# Coop vs Oddjobs Claude Adapter

Coop provides the **session layer** (spawn, monitor, encode input).
Oddjobs provides the **orchestration layer** (jobs, decisions, stuck recovery).

```
Before:  Engine → ClaudeAgentAdapter → TmuxAdapter → tmux → Claude Code
                          ↓
                   Watcher (file)  →  session log JSONL

After:   Engine → CoopAdapter → HTTP/gRPC → coop → PTY → Claude Code
                                               ↓
                                        multi-tier detection
```


## Session Management

| Capability           | OJ | Coop | Notes                           |
| -------------------- | -- | ---- | ------------------------------- |
| Spawn                | ✓  | ✓    | tmux session vs native PTY      |
| Terminal rendering   | ✓  | ✓+   | raw text vs VTE-parsed screen   |
| Input injection      | ✓  | ✓    | `send-keys` vs PTY write        |
| Kill                 | ✓  | ✓    |                                 |
| Liveness / exit code | ✓  | ✓    | `pane_dead` vs waitpid          |
| Output preservation  | ✓  | ✓    | `remain-on-exit` vs ring buffer |


## State Detection

| Signal              | OJ | Coop | Notes                                     |
| ------------------- | -- | ---- | ----------------------------------------- |
| Notification hook   | ✓  | ✓    | CLI callback vs FIFO pipe                 |
| PreToolUse hook     | ✓  | ✓    | CLI callback vs FIFO pipe                 |
| PostToolUse hook    | ✗  | ✓    | Coop uses for Working signal              |
| Stop hook           | ✓  | ✓    | OJ: exit gate. Coop: detection + gating   |
| Session log watcher | ✓  | ✓    | OJ: 5s poll fallback. Coop: Tier 2        |
| Stdout JSONL        | ✗  | ✓    | Tier 3                                    |
| Process monitor     | ✓  | ✓    | Tier 4                                    |
| Screen parsing      | ✗  | ✓    | Tier 5: setup dialogs, trust, idle prompt |


## Prompt Handling

| Prompt                    | OJ | Coop | Notes                                       |
| ------------------------- | -- | ---- | ------------------------------------------- |
| Permission detection      | ✓  | ✓    | Both via Notification hook                  |
| Permission response       | ✓  | ✓    | OJ: numbered option. Coop: `{n}\r`          |
| AskUser detection         | ✓  | ✓    | Both via PreToolUse hook                    |
| AskUser response          | ✓  | ✓+   | Coop adds multi-question encoding           |
| Plan detection            | ✓  | ✓    | Both via PreToolUse hook                    |
| Plan response             | ✓  | ✓    | OJ: arrow keys. Coop: `{n}\r`               |
| Setup dialog detection    | ✗  | ✓    | Tier 5 screen classification                |
| Setup dialog response     | ✗  | ✓    | `{n}\r`                                     |
| Prompt context extraction | ✗  | ✓    | tool, input, options, questions, ready flag |


## Startup Prompts

| Prompt             | OJ | Coop | Notes                                                        |
| ------------------ | -- | ---- | ------------------------------------------------------------ |
| Bypass permissions | ✓  | ✓    | OJ: auto-accepts. Coop: suppresses idle, reports to consumer |
| Workspace trust    | ✓  | ✓    | Same                                                         |
| Login/onboarding   | ✓  | ✓    | OJ: kills session. Coop: reports as `Prompt(Setup)`          |

Coop does not auto-respond to startup prompts. It detects them (to suppress
false idle signals) and reports them. The orchestrator responds via the API.


## Idle Detection

| Aspect                  | OJ | Coop | Notes                                                               |
| ----------------------- | -- | ---- | ------------------------------------------------------------------- |
| Grace timer             | ✓  | ✗    | OJ: 60s two-phase confirmation. Coop: tier-priority resolution only |
| Self-trigger prevention | ✓  | ✗    | OJ: suppresses auto-resume 60s after nudge                          |

Coop has no grace timer. The composite detector resolves competing idle/working
signals via tier priority — higher-confidence tiers override lower ones, and
lower-confidence tiers can only escalate state, never downgrade. Between tool
calls, the log (Tier 2) typically continues to report `Working`, preventing
false idle signals. Oddjobs' watcher only has one tier, so it needs the grace
timer to distinguish "idle between tools" from "actually idle."


## Input Encoding

| Action         | OJ | Coop | Notes                                             |
| -------------- | -- | ---- | ------------------------------------------------- |
| Nudge          | ✓  | ✓    | OJ clears partial input (Esc+pause+Esc) first     |
| Input clearing | ✓  | ✗    | Coop relies on consumer sending at the right time |


## Session Resume

| Aspect              | OJ | Coop |
| ------------------- | -- | ---- |
| Resume conversation | ✓  | ✓    |
| Session ID tracking | ✓  | ✓    |
| Log offset recovery | ✗  | ✓    |
| Daemon reconnect    | ✓  | ✓    |
| Suspend/resume      | ✓  | ✗    |

Oddjobs has job-level suspension (`StepStatus::Suspended`) that pauses state
processing while keeping the tmux session alive. Coop has no equivalent;
consumers ignore events to achieve the same effect.


## Hooks

Both use Notification and PreToolUse hooks for detection, with different
transports (CLI callback vs FIFO). Settings coexist since matchers don't
conflict.

| Hook             | OJ                                 | Coop                             |
| ---------------- | ---------------------------------- | -------------------------------- |
| **Stop**         | Exit gate (`oj emit agent:signal`) | Detection (FIFO) + gating (HTTP) |
| **Notification** | `oj agent hook notify`             | FIFO → Tier 1                    |
| **PreToolUse**   | `oj agent hook pretooluse`         | FIFO → Tier 1                    |
| **PostToolUse**  | Not used                           | FIFO → Tier 1 Working            |
| **SessionStart** | Prime scripts                      | Not used                         |

The Stop hook serves different purposes: OJ **prevents** exit until the
orchestrator is satisfied; coop uses it for **detection** and optional
**gating** (configurable via `StopConfig`).


## Out of Scope for Coop

These remain orchestrator-level concerns in oddjobs:

| Component | Description |
| --- | --- |
| Decision system | Human-in-the-loop records with numbered options, context, resolution tracking |
| Job lifecycle | Multi-step workflows, suspend/resume/cancel |
| Workspace management | Git worktrees, directory setup, cleanup |
| Agent signaling | `complete`/`escalate`/`continue` signals via `oj emit agent:signal` |
| Settings injection | Per-agent `claude-settings.json` with hooks and permissions |
| Stuck recovery policy | Nudge attempts, escalation thresholds, retry limits |
| Desktop notifications | Alert humans when decisions are needed |
