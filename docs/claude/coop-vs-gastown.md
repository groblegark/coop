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

| Signal                | GT | Coop |
| --------------------- | -- | ---- |
| Agent self-reporting  | ✓  | ✗    |
| Notification hook     | ✗  | ✓    |
| PreToolUse hook       | ✓  | ✓+   |
| PostToolUse hook      | ✓  | ✓    |
| Stop hook             | ✓  | ✓+   |
| SessionStart hook     | ✓  | ✓    |
| UserPromptSubmit hook | ✓  | ✓    |
| Session log watcher   | ✗  | ✓    |
| Stdout JSONL          | ✗  | ✓    |
| Process monitor       | ✓  | ✓    |
| Screen parsing        | ✗  | ✓    |
| Health check pings    | ✓  | ✗    |
| Idle detection        | ✓  | ✓+   |
| Active health pings   | ✓  | ✗    |


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

| Prompt             | GT | Coop |
| ------------------ | -- | ---- |
| Bypass permissions | ✓  | ✓    |
| Workspace trust    | ✗  | ✓    |
| Login/onboarding   | ✗  | ✓    |



## Input Encoding

| Action              | GT | Coop |
| ------------------- | -- | ---- |
| Nudge               | ✓  | ✓+   |
| Permission respond  | ✗  | ✓    |
| AskUser respond     | ✗  | ✓    |
| Plan respond        | ✗  | ✓    |
| Input debouncing    | ✓  | ✓    |


## Session Resume

| Aspect                | GT | Coop |
| --------------------- | -- | ---- |
| Resume conversation   | ✓  | ✓    |
| Predecessor discovery | ✓  | ✗    |
| Log offset recovery   | ✗  | ✓    |
| Credential switch     | ✗  | ✓+   |
| Multi-account         | ✓  | ✓    |


## Hooks & Settings Merging

GT passes hooks, permissions, env, plugins, and MCP servers via `--agent-config`.
Coop appends its detection hooks on top (GT first, coop second) and writes a single merged settings file.
