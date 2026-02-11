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

| Capability           | OJ | Coop | Notes             |
| -------------------- | -- | ---- | ----------------- |
| Spawn                | ✓  | ✓    |                   |
| Terminal rendering   | ✓  | ✓+   | VTE-parsed screen |
| Input injection      | ✓  | ✓    |                   |
| Kill                 | ✓  | ✓    |                   |
| Liveness / exit code | ✓  | ✓    |                   |
| Output preservation  | ✓  | ✓    |                   |


## State Detection

| Signal              | OJ | Coop |
| ------------------- | -- | ---- |
| Notification hook   | ✓  | ✓    |
| PreToolUse hook     | ✓  | ✓    |
| PostToolUse hook    | ✗  | ✓    |
| UserPromptSubmit    | ✗  | ✓    |
| Stop hook           | ✓  | ✓    |
| SessionStart hook   | ✓  | ✓    |
| Session log watcher | ✓  | ✓    |
| Stdout JSONL        | ✗  | ✓    |
| Process monitor     | ✓  | ✓    |
| Screen parsing      | ✗  | ✓    |


## Prompt Handling

| Prompt                    | OJ | Coop | Notes                        |
| ------------------------- | -- | ---- | ---------------------------- |
| Permission detection      | ✓  | ✓    |                              |
| Permission response       | ✓  | ✓    |                              |
| AskUser detection         | ✓  | ✓    |                              |
| AskUser response          | ✓  | ✓+   | Adds multi-question encoding |
| Plan detection            | ✓  | ✓    |                              |
| Plan response             | ✓  | ✓    |                              |
| Setup dialog detection    | ✗  | ✓    | Tier 5 screen classification |
| Setup dialog response     | ✗  | ✓    |                              |
| Prompt context extraction | ✗  | ✓    |                              |


## Startup Prompts

| Prompt             | OJ | Coop | Notes                                    |
| ------------------ | -- | ---- | ---------------------------------------- |
| Bypass permissions | ✓  | ✓    |                                          |
| Workspace trust    | ✓  | ✓    |                                          |
| Login/onboarding   | ✗  | ✓    | Extracts login link, exposes via API     |

With `--groom manual`, coop reports prompts without auto-responding. With
`--groom auto` (default), coop auto-dismisses interactive dialogs but not
text-based startup prompts.


## Idle Detection

| Aspect                  | OJ | Coop | Notes                          |
| ----------------------- | -- | ---- | ------------------------------ |
| Grace timer             | ✓  | ✗    | 60s two-phase confirmation     |
| Self-trigger prevention | ✓  | ✗    | Suppresses 60s after nudge     |

Coop has no grace timer. The composite detector resolves competing idle/working
signals via tier priority — higher-confidence tiers override lower ones, and
lower-confidence tiers can only escalate state, never downgrade. Between tool
calls, the log (Tier 2) typically continues to report `Working`, preventing
false idle signals. Oddjobs' watcher only has one tier, so it needs the grace
timer to distinguish "idle between tools" from "actually idle."


## Input Encoding

| Action           | OJ | Coop | Notes                              |
| ---------------- | -- | ---- | ---------------------------------- |
| Nudge            | ✓  | ✓    |                                    |
| Delay scaling    | ✗  | ✓    |                                    |
| Nudge retry      | ✗  | ✓    | Resend `\r` if no state transition |
| Input clearing   | ✓  | ✗    |                                    |
| Input debouncing | ✗  | ✓    | 200ms min gap between deliveries   |


## Session Resume

| Aspect               | OJ | Coop | Notes                               |
| -------------------- | -- | ---- | ----------------------------------- |
| Resume conversation  | ✓  | ✓    |                                     |
| Session ID tracking  | ✓  | ✓    |                                     |
| Log offset recovery  | ✗  | ✓    |                                     |
| Daemon reconnect     | ✓  | ✓    |                                     |
| Suspend/resume       | ✓  | ✗    |                                     |
| Credential switch    | ✗  | ✓    | Profiles with rate-limit rotation   |
| Transcript snapshots | ✗  | ✓    | Snapshots on compaction             |

Oddjobs has job-level suspension (`StepStatus::Suspended`) that pauses state
processing while keeping the tmux session alive. Coop has no equivalent;
consumers ignore events to achieve the same effect.


## Hooks & Settings Merging

OJ passes hooks, permissions, and MCP servers via `--agent-config`. Coop
appends its detection hooks on top (OJ first, coop second).

| Hook             | OJ                                 | Coop                             |
| ---------------- | ---------------------------------- | -------------------------------- |
| **Stop**         | Exit gate (`oj emit agent:signal`) | Detection (FIFO) + gating (HTTP) |
| **Notification** | `oj agent hook notify`             | FIFO → Tier 1                    |
| **PreToolUse**   | `oj agent hook pretooluse`         | FIFO → Tier 1                    |
| **PostToolUse**  | Not used                           | FIFO → Tier 1 Working            |
| **UserPromptSubmit** | Not used                       | FIFO → Tier 1 Working            |
| **SessionStart** | Prime scripts                      | Context injection                |

The Stop hook serves different purposes: OJ **prevents** exit until the
orchestrator is satisfied; coop uses it for **detection** and optional
**gating** (configurable via `StopConfig`).


## Out of Scope for Coop

These remain orchestrator-level concerns in oddjobs:

| Component             | Description |
| --------------------- | --- |
| Decision system       | Human-in-the-loop records with numbered options, context, resolution tracking |
| Job lifecycle         | Multi-step workflows, suspend/resume/cancel |
| Workspace management  | Git worktrees, directory setup, cleanup |
| Stuck recovery policy | Nudge attempts, escalation thresholds, retry limits |
| Desktop notifications | Alert humans when decisions are needed |
