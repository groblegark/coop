# Epic: Yoink Graceful Shutdown from groblegark/coop

Cherry-pick and adapt the graceful shutdown feature from `groblegark/coop@62423119`.

## Context

When coop receives SIGTERM while the agent is mid-task, it immediately SIGHUPs the
process group. The agent has no chance to finish or checkpoint. The fork added a
"graceful drain" mode: send Escape to abort in-flight work, wait for idle, then kill.
This is valuable for k8s pod termination and `--resume` reliability.

**Source commit:** `groblegark/coop@62423119`
**Files touched:** `config.rs` (+6), `session.rs` (+63/-5)

## Plan

### Step 1: Cherry-pick and resolve conflicts

```
git fetch https://github.com/groblegark/coop.git main
git cherry-pick --no-commit 62423119
```

The cherry-pick will conflict against upstream `session.rs` because:
- The select loop has new branches (screen debounce, option enrichment)
- Branch numbering shifted (shutdown was branch 6, still is)
- `BackendInput` type usage may differ

Resolve manually — the new code goes between the existing idle timeout (branch 5)
and the shutdown signal (branch 6). The shutdown branch becomes branch 8, with
the two new drain branches at 6 and 7.

### Step 2: Fix `BackendInput` type mismatch

The fork sends raw `Bytes` to `backend_input_tx`:
```rust
let esc = Bytes::from_static(b"\x1b");
let _ = self.backend_input_tx.send(esc).await;
```

Upstream's channel is `mpsc::Sender<BackendInput>` where `BackendInput` is an enum.
Change to:
```rust
let _ = self.backend_input_tx.send(BackendInput::Write(Bytes::from_static(b"\x1b"))).await;
```

### Step 3: Route Escape through the InputEvent channel

The fork bypasses the `InputEvent` pipeline, which means:
- Escape bytes aren't counted in `lifecycle.bytes_written`
- No `input_activity` notification (affects enter-retry monitor)
- Races with queued `DeliveryGate`-gated nudge/respond deliveries

**Fix:** Send the Escape via `self.consumer_input_rx`'s sender instead. But the
session loop only holds the rx side, not the tx.

Preferred approach: keep the direct `backend_input_tx` write (the session loop
_is_ the consumer of `InputEvent` — it's the one that forwards to `backend_input_tx`
anyway). But add the byte-counting and input_activity notification inline:

```rust
// In the drain escape branch:
let esc = Bytes::from_static(b"\x1b");
self.app_state.lifecycle.bytes_written.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
self.app_state.input_activity.notify_waiters();
let _ = self.backend_input_tx.send(BackendInput::Write(esc)).await;
```

This preserves the accounting without needing to route through the channel.

The `DeliveryGate` is for external callers (nudge/respond handlers). The session
loop is _internal_ — it doesn't need to serialize against external deliveries
because the agent is being shut down. Escape during drain shouldn't contend with
new nudge requests.

### Step 4: Replace lock-based state read with local tracking

The fork reads `agent_state` under an async RwLock in the shutdown branch:
```rust
let current_state = self.app_state.driver.agent_state.read().await;
let is_idle = matches!(*current_state, AgentState::WaitingForInput);
```

The session loop already processes all state transitions in branch 3. Track the
last known state in a local variable instead:

```rust
// At top of run():
let mut last_state = AgentState::Starting;

// In branch 3 (detector state changes), after `*current = detected.state.clone()`:
last_state = detected.state.clone();

// In shutdown branch:
let is_idle = matches!(last_state, AgentState::WaitingForInput);
```

Similarly, the drain-complete check in branch 3 should use `detected.state`
directly (it already does via `matches!`) rather than re-reading the lock.

### Step 5: Add config knob

The fork adds `graceful_shutdown_timeout()` to `Config`. This is clean and follows
existing patterns. Apply as-is:

```rust
/// Graceful shutdown timeout: how long to wait for the agent to reach idle
/// after receiving SIGTERM before force-killing (0 = disabled, immediate kill).
pub fn graceful_shutdown_timeout(&self) -> Duration {
    env_duration_secs("COOP_GRACEFUL_SHUTDOWN_SECS", 20)
}
```

### Step 6: Add tests

Add three tests to `session_tests.rs`:

1. **`graceful_drain_kills_when_already_idle`** — Agent is `sleep 60`, cancel
   shutdown immediately. With `COOP_GRACEFUL_SHUTDOWN_SECS=5`, should exit
   promptly (not wait 5s). Verifies the "already idle → immediate kill" path
   doesn't regress the existing `shutdown_cancels_session` behavior.

2. **`graceful_drain_timeout_force_kills`** — Long-running agent (`sleep 60`,
   no detector, never goes idle). Set `COOP_GRACEFUL_SHUTDOWN_SECS=1`. Cancel
   shutdown. Should exit within ~1-2s (drain deadline). Verifies the force-kill
   path.

3. **`graceful_drain_disabled_when_zero`** — Set `COOP_GRACEFUL_SHUTDOWN_SECS=0`.
   Should behave exactly like pre-feature: immediate SIGHUP on shutdown. Same
   as existing `shutdown_cancels_session` but explicit about the knob.

These are unit-level tests using `NativePty::spawn` + `Session::new`, matching
the existing test pattern. No claudeless/e2e needed for this — the behavior is
entirely within the session select loop.

### Step 7: Run `make check`

Ensure fmt, clippy, quench, build, and test all pass.

### Step 8: Commit

```
feat(session): graceful shutdown — wait for idle before killing agent

Cherry-picked from groblegark/coop@62423119 with adaptations:
- Route drain Escape through BackendInput::Write with byte accounting
- Track last state locally instead of re-reading async RwLock
- Add unit tests for drain-idle, drain-timeout, and disabled paths

When SIGTERM arrives mid-task, coop enters drain mode: sends Escape
every 2s to abort in-flight work, waits for idle, then SIGHUPs.
Configurable via COOP_GRACEFUL_SHUTDOWN_SECS (default 20, 0=disabled).
```
