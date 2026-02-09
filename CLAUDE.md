# Coop

Coop is a terminal session manager for AI coding agents. It wraps agent CLIs in a PTY (or compatibility layer), monitors their state, and exposes a gRPC API for orchestration.

## Build & Test

```bash
make check    # fmt + clippy + quench + build + test
make ci       # full pre-release (adds audit + deny)
cargo test    # unit tests only
```

### Manual testing with claudeless

```bash
make try-claudeless SCENARIO=crates/cli/tests/scenarios/claude_hello.toml
make try-claudeless SCENARIO=crates/cli/tests/scenarios/claude_tool_use.toml
make try-claudeless SCENARIO=crates/cli/tests/scenarios/claude_ask_user.toml
```

Opens a browser terminal running coop → claudeless with the given scenario. Useful for debugging hook detection, state transitions, and TUI rendering.

**Important**: You cannot run `try-claudeless` yourself — it opens a browser terminal. When debugging claudeless scenarios (new or failing), ask the human to run `try-claudeless` and report what they see.

## Code Conventions

- License: BUSL-1.1; every source file needs the SPDX header
- Clippy: `unwrap_used`, `expect_used`, `panic` are denied; use `?`, `anyhow::bail!`, or `.ok()`
- Unsafe: forbidden workspace-wide
- Tests: use `-> anyhow::Result<()>` return type instead of unwrap

## Architecture

- `run::prepare()` sets up the full session (driver, backend, servers) and returns a `PreparedSession` with access to `AppState` before the session loop starts. `run::run()` is the simple wrapper that calls `prepare().run()`.
- Claude driver detection has three tiers: Tier 1 (hook FIFO), Tier 2 (session log), Tier 3 (stdout JSONL). Hooks are the primary detection path.
- Session artifacts (FIFO pipe, settings) live at `$XDG_STATE_HOME/coop/sessions/<session-id>/` for debugging and recovery.
- Integration tests use claudeless (scenario-driven Claude CLI simulator). Tests call `run::prepare()`, subscribe to state broadcasts, spawn the session, `wait_for` expected states, then cancel shutdown.

## Working Style

- Use `AskUserQuestion` frequently — ask before making architectural choices, when multiple approaches exist, or when unsure about intent. A quick question is cheaper than rework.
- Prefer end-to-end testing through the real `run()` codepath over manual library wiring. Tests should be trivial to read.
- Keep agent-specific code (Claude, Gemini) in `driver/<agent>/`; `run.rs` and `session.rs` should stay agent-agnostic.

## Directory Structure

```toc
crates/cli/               # Single crate (binary + lib)
├── src/
│   ├── main.rs            # CLI, startup
│   ├── lib.rs             # Library root (re-exports modules)
│   ├── run.rs             # prepare() + run() session entrypoint
│   ├── error.rs           # ErrorCode enum
│   ├── event.rs           # OutputEvent, StateChangeEvent, InputEvent, HookEvent
│   ├── screen.rs          # Screen, ScreenSnapshot
│   ├── ring.rs            # RingBuffer
│   ├── pty/
│   │   ├── mod.rs         # Backend trait
│   │   ├── nbio.rs        # Non-blocking I/O helpers (PtyFd, AsyncFd)
│   │   └── spawn.rs       # NativePty backend (forkpty + exec)
│   └── driver/
│       ├── mod.rs          # AgentState, Detector, NudgeEncoder traits
│       └── jsonl_stdout.rs # JsonlParser
└── tests/
    ├── pty_backend.rs           # Integration tests for PTY backend
    ├── claude_integration.rs    # E2E tests via claudeless
    └── scenarios/               # Claudeless scenario fixtures
DESIGN.md                   # Full design spec
ROADMAP.md                  # Phased dependency graph
```

## Development

### Quick checks

```sh
make check    # fmt, clippy, quench, build, test
```

### Conventions

- License: BUSL-1.1, Copyright Alfred Jean LLC
- All source files need SPDX license header
- Rust 1.92+: native `async fn` in traits, no `async_trait` macro
- Unit tests in `*_tests.rs` files with `#[cfg(test)] #[path = "..."] mod tests;`

## Commits

Use conventional commit format: `type(scope): description`

Types: feat, fix, chore, docs, test, refactor

## Landing the Plane

Before completing work:

1. Run `make check` — all fmt, clippy, quench, build, and test steps must pass
2. Ensure new source files have SPDX license headers
3. Commit with conventional commit format
