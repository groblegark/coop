# Coop

A terminal session manager for AI coding agents. Spawns agent CLIs on a PTY, classifies agent state via structured data, and serves everything over your choice of HTTP, WebSocket, or gRPC.

Coop replaces tmux/screen-based agent orchestration with a proper API. Instead of `capture-pane` and `send-keys`, consumers get structured endpoints for screen state, agent state detection, nudging idle agents, and answering prompts.

## Install

```bash
scripts/install    # builds and installs to ~/.local/bin/
```

Or build manually:

```bash
cargo build --release
```

## Usage

```bash
# Claude with structured detection
coop --agent claude --port 8080 -- claude --dangerously-skip-permissions

# Serve on a Unix socket
coop --agent claude --socket /tmp/coop.sock -- claude

# Dumb PTY server (no driver)
coop --port 8080 -- /bin/bash

# Attach to an existing tmux session
coop --agent claude --attach tmux:my-session --port 8080

# Enable gRPC alongside HTTP
coop --agent claude --port 8080 --port-grpc 9090 -- claude

# Resume a previous Claude session
coop --agent claude --port 8080 --resume <session-id> -- claude
```

## API

Once coop is running, consumers interact with agents over HTTP or gRPC:

```bash
# Check agent state
curl localhost:8080/api/v1/agent/state

# Give the agent a task
curl -X POST localhost:8080/api/v1/agent/nudge \
  -d '{"message": "Fix the login bug"}'

# Accept a permission prompt
curl -X POST localhost:8080/api/v1/agent/respond \
  -d '{"accept": true}'

# View the terminal screen
curl localhost:8080/api/v1/screen/text

# Stream events over WebSocket
websocat ws://localhost:8080/ws?mode=state
```

### Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/health` | Health check |
| GET | `/api/v1/screen` | Rendered screen (JSON) |
| GET | `/api/v1/screen/text` | Plain text screen |
| GET | `/api/v1/output` | Raw ring buffer |
| GET | `/api/v1/status` | Process status |
| POST | `/api/v1/input` | Send text input |
| POST | `/api/v1/input/keys` | Send key sequences |
| POST | `/api/v1/resize` | Resize terminal |
| POST | `/api/v1/signal` | Signal child process |
| GET | `/api/v1/agent/state` | Agent state + prompt context |
| POST | `/api/v1/agent/nudge` | Deliver message to idle agent |
| POST | `/api/v1/agent/respond` | Answer agent prompt |
| GET | `/ws` | WebSocket (raw, screen, state, or all) |

gRPC is also available when `--port-grpc` is set, mirroring the HTTP endpoints with streaming RPCs for output, screen, and state.

## Agent Drivers

Coop uses structured data sources (not screen scraping) to classify agent state:

| Agent | Maturity | Hooks | Session log | Stdout JSONL | Process |
|-------|----------|-------|-------------|--------------|---------|
| `claude` | Beta | PostToolUse, Stop | `~/.claude/sessions/` | `--print --output-format stream-json` | Yes |
| `codex` | TODO | -- | -- | `--json` | Yes |
| `gemini` | Pre-alpha | AfterTool, SessionEnd | `~/.gemini/tmp/` | `stream-json` | Yes |
| `unknown` | Experimental | -- | -- | -- | Yes |

Agent states: `starting`, `working`, `waiting_for_input`, `permission_prompt`, `plan_prompt`, `ask_user`, `error`, `alt_screen`, `exited`, `unknown`.

## Development

### Requirements

- **Rust 1.92+** — install via [rustup](https://rustup.rs/)
- **protoc** — Protocol Buffers compiler (used by `prost-build` for gRPC codegen)
  - macOS: `brew install protobuf`
  - Debian/Ubuntu: `apt install protobuf-compiler`
- **[quench](https://github.com/nicholasgasior/quench)** — fast linting tool for quality signals, used by `make check`
- **[claudeless](https://github.com/alfredjeanlab/claudeless)** — Claude CLI mock, used for integration tests and manual testing

Optional:

- **Docker** — used for `make test-docker` and `try-docker-*` targets
- **Claude CLI** / **Gemini CLI** — only needed for `make try-claude` / `make try-gemini` manual testing

### Commands

```bash
make check    # fmt + clippy + quench + build + test
make ci       # full pre-release (adds audit + deny)
cargo test    # unit tests only
```

### Manual testing

```bash
# Launch coop + claudeless in a browser terminal (requires claudeless)
make try-claudeless SCENARIO=crates/cli/tests/scenarios/claude_hello.toml

# Launch coop + real agent CLI in a browser terminal
make try-claude     # requires claude CLI
make try-gemini     # requires gemini CLI

# Docker variants (requires Docker)
make try-docker-claudeless SCENARIO=claude_hello.toml
make try-docker-claude
make try-docker-gemini
make test-docker    # run Docker e2e tests
```

## License

Licensed under the Business Source License 1.1
Copyright (c) Alfred Jean LLC
See LICENSE for details.
