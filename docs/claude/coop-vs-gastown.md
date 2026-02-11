# Coop vs Gastown

Coop provides the **session layer** (spawn, monitor, encode input).
Gastown provides the **orchestration layer** (polecats, witness, beads,
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

**Key difference**: Gastown agents are autonomous workers that self-report
milestones (`gt done`, `gt help`). Coop provides passive observation with
structured state classification. These are complementary — Gastown's
self-reporting protocol would run inside a coop-managed PTY.


## Session Management

| Capability           | GT | Coop |
| -------------------- | -- | ---- |
| Spawn                | ✓  | ✓    |
| Terminal rendering   | ✓  | ✓+   |
| Input injection      | ✓  | ✓    |
| Kill                 | ✓  | ✓    |
| Liveness / exit code | ✓  | ✓    |
| Output preservation  | ✓  | ✓    |
| Input serialization  | ✓  | ✓    |


## State Detection

Gastown uses hooks for **workflow orchestration** (context injection, mail, decisions).
Coop uses hooks for **state detection**.

The hook types overlap but serve completely different purposes.

| Signal                | GT | Coop | Notes                         |
| --------------------- | -- | ---- | ----------------------------- |
| Agent self-reporting  | ✓  | ✗    | Runs inside coop PTY          |
| Notification hook     | ✗  | ✓    |                               |
| PreToolUse hook       | ✓  | ✓+   | Adds prompt detection         |
| PostToolUse hook      | ✓  | ✓+   | Adds Working signal           |
| Stop hook             | ✓  | ✓+   | Adds state detection          |
| SessionStart hook     | ✓  | ✓    |                               |
| UserPromptSubmit hook | ✓  | ✓+   | Adds Working signal           |
| Session log watcher   | ✗  | ✓    |                               |
| Stdout JSONL          | ✗  | ✓    |                               |
| Process monitor       | ✓  | ✓    |                               |
| Screen parsing        | ✗  | ✓    |                               |
| Health check pings    | ✓  | ✗    |                               |


## Prompt Handling

Gastown agents run `--dangerously-skip-permissions` and don't encounter permission prompts during normal operation.
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

| Prompt             | GT | Coop | Notes                                    |
| ------------------ | -- | ---- | ---------------------------------------- |
| Bypass permissions | ✓  | ✓    |                                          |
| Workspace trust    | ✗  | ✓    |                                          |
| Login/onboarding   | ✗  | ✓    | Extracts login link, exposes via API     |

With `--groom manual`, coop reports prompts without auto-responding. With
`--groom auto` (default), coop auto-dismisses interactive dialogs but not
text-based startup prompts.


## Idle / Stuck Detection

| Aspect                  | GT | Coop | Notes                            |
| ----------------------- | -- | ---- | -------------------------------- |
| Passive state detection | ✗  | ✓    | Multi-tier composite detector    |
| Active health pings     | ✓  | ✗    |                                  |

GT's deacon actively probes agents. Coop passively observes and reports state;
the consumer decides recovery strategy. GT's active probing is an
orchestrator-level policy that would consume coop's state events instead of
sending health check pings.


## Input Encoding

| Action              | GT | Coop |
| ------------------- | -- | ---- |
| Nudge               | ✓  | ✓+   |
| Permission respond  | ✗  | ✓    |
| AskUser respond     | ✗  | ✓    |
| Plan respond        | ✗  | ✓    |
| Input debouncing    | ✓  | ✓    |


## Session Resume

| Aspect                | GT | Coop | Notes                             |
| --------------------- | -- | ---- | --------------------------------- |
| Resume conversation   | ✓  | ✓    |                                   |
| Predecessor discovery | ✓  | ✗    |                                   |
| Log offset recovery   | ✗  | ✓    |                                   |
| Credential switch     | ✗  | ✓+   | Profiles with rate-limit rotation |
| Multi-account         | ✓  | ✓    |                                   |

GT uses beacons (`[GAS TOWN] recipient <- sender • timestamp • topic`) for predecessor discovery in Claude's `/resume` picker.
Coop's `--resume` discovers the log file and passes `--resume <id>` to Claude.
These are complementary — GT's beacon injection would work inside a coop-managed session.


## Context Continuity

Coop and Gastown each address context loss, but at different boundaries.

**Coop — transcript snapshots** preserve raw conversation history within a
session. When Claude compacts its context window, coop copies the session log
to a numbered snapshot file. Clients retrieve snapshots via HTTP, gRPC, or
WebSocket, with cursor-based catchup for incremental sync.

**Gastown — seance** recovers context across sessions. When a rig is "cold"
(no activity for >24h), `gt prime` spawns `claude --fork-session --resume <id>`
against the predecessor session, asks a structured handoff prompt, and injects
the summary into the successor's startup context. Results are cached for 1 hour.

| Aspect | Coop (Transcript) | GT (Seance) |
|--------|-------------------|-------------|
| Boundary | Intra-session (compaction) | Inter-session (handoff) |
| Trigger | `SessionStart` hook with `source="compact"` | `gt prime` on cold rig (>24h idle) |
| Output | Raw JSONL snapshot (full conversation) | LLM-generated 5-point summary (<500 words) |
| Storage | `sessions/<id>/transcripts/{N}.jsonl` (persistent) | `.beads-wisp/seance-cache.json` (1h TTL) |
| Access | HTTP/gRPC/WS APIs | Consumed internally by `gt prime` |
| Consumer | Orchestrators, UIs, external clients | The successor agent |

Note: seance uses `--fork-session --resume` which only loads the
post-compaction context. Transcript snapshots preserve pre-compaction history
that fork-session cannot see.


## Hooks & Settings Merging

GT passes hooks, permissions, env, plugins, and MCP servers via `--agent-config`.
Coop appends its detection hooks on top (GT first, coop second) and writes a
single merged settings file. MCP servers are written to the session dir and
passed via `--mcp-config`.


## Out of Scope for Coop

These remain orchestrator-level concerns in Gastown:

| Component                   | Description |
| --------------------------- | ----------- |
| Config bead materialization | Merges settings from structured metadata layers (passed to coop via `--agent-config`) |
| Witness protocol            | POLECAT_DONE, HELP, MERGED, RATE_LIMITED messages (runs inside coop PTY) |
| Deacon health policy        | Stuck recovery: thresholds, cooldowns, force-kill (subscribes to coop state events) |
| Beacon / predecessor        | Session continuity for `/resume` picker |
| Merge queue (refinery)      | Sequential rebase, conflict → fresh polecat |
| Polecat lifecycle           | Spawn, work, `gt done`, die |
| Work assignment             | `gt sling`, beads, hook-driven context injection |
| Inter-agent mail            | Messaging between polecats, witness, deacon |


## Migration Path

1. **Phase 1 — opt-in**: Ship coop binary, add `CoopBackend` behind `--session-backend=coop` flag, tmux path unchanged
2. **Phase 2 — default**: New polecats use coop, subscribe to state events for health monitoring (replaces ping cycle), tmux as fallback
3. **Phase 3 — remove tmux**: Delete `internal/tmux/` (~1,800 lines), all sessions through coop
