# Coop

A terminal session manager for AI coding agents. Spawns agent CLIs on a PTY, classifies agent state via structured data, and serves everything over HTTP + WebSocket + gRPC.

Coop replaces tmux/screen-based agent orchestration with a proper API. Instead of `capture-pane` and `send-keys`, consumers get structured endpoints for screen state, agent state detection, nudging idle agents, and answering prompts.

### Building

```bash
cargo build
make check   # Run all CI checks (fmt, clippy, quench, test, build)
```

### Usage

```bash
# Claude with structured detection
coop --agent-type claude --port 8080 -- claude --dangerously-skip-permissions

# Codex on Unix socket
coop --agent-type codex --socket /tmp/coop.sock -- codex

# Dumb PTY server (no driver)
coop --port 8080 -- /bin/bash

# Poll agent state
curl localhost:8080/api/v1/agent/state

# Nudge an idle agent
curl -X POST localhost:8080/api/v1/agent/nudge \
  -d '{"message": "Fix the login bug"}'
```

## License

Licensed under the Business Source License 1.1
Copyright (c) Alfred Jean LLC
See LICENSE for details.
