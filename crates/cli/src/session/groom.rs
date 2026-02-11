// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Prompt enrichment and auto-dismiss logic.
//!
//! These tasks run as detached spawned futures, polling the screen buffer
//! for option labels and auto-dismissing disruption prompts.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tracing::{debug, warn};

use crate::config::{Config, GroomLevel};
use crate::driver::{disruption_option, AgentState, NudgeStep, OptionParser, PromptKind};
use crate::event::{InputEvent, PromptOutcome, TransitionEvent};
use crate::transport::Store;

/// Spawn deferred option enrichment for Permission/Plan prompts.
///
/// Hook events fire before the screen renders numbered options,
/// so we wait briefly for the PTY output to catch up, then parse
/// options from the screen and re-broadcast the enriched state.
pub(super) fn spawn_enrichment(store: &Arc<Store>, state_seq: u64, parser: &OptionParser) {
    let app = Arc::clone(store);
    let parser = Arc::clone(parser);
    tokio::spawn(enrich_prompt_options(app, state_seq, parser));
}

/// Wait for the screen to render prompt options, parse them, and re-broadcast the enriched prompt context.
///
/// Retries up to `MAX_ATTEMPTS` times, then falls back to universal Accept/Cancel options
/// that encode to Enter/Esc.
async fn enrich_prompt_options(app: Arc<Store>, expected_seq: u64, parser: OptionParser) {
    const MAX_ATTEMPTS: u32 = 10;
    const POLL_INTERVAL: Duration = Duration::from_millis(200);

    let mut last_snap_lines = 0usize;

    for _ in 0..MAX_ATTEMPTS {
        tokio::time::sleep(POLL_INTERVAL).await;

        // Bail if the state has changed since we spawned.
        let current_seq = app.driver.state_seq.load(std::sync::atomic::Ordering::Acquire);
        if current_seq != expected_seq {
            return;
        }

        let screen = app.terminal.screen.read().await;
        let snap = screen.snapshot();
        drop(screen);
        last_snap_lines = snap.lines.len();

        let options = parser(&snap.lines);
        if !options.is_empty() {
            let mut agent = app.driver.agent_state.write().await;

            // Re-check seq under the write lock.
            let current_seq = app.driver.state_seq.load(std::sync::atomic::Ordering::Acquire);
            if current_seq != expected_seq {
                return;
            }

            if let AgentState::Prompt { ref mut prompt } = *agent {
                if matches!(prompt.kind, PromptKind::Permission | PromptKind::Plan) {
                    prompt.options = options;
                    prompt.ready = true;

                    let next = agent.clone();
                    drop(agent);

                    let last_message = app.driver.last_message.read().await.clone();
                    let _ = app.channels.state_tx.send(TransitionEvent {
                        prev: next.clone(),
                        next,
                        seq: expected_seq,
                        cause: "ready".to_owned(),
                        last_message,
                    });
                }
            }
            return;
        }
    }

    // All retries exhausted — set fallback options so API consumers have
    // something to present (Enter for accept, Esc for cancel).
    debug!(last_snap_lines, "prompt option enrichment: setting fallback options");

    let mut agent = app.driver.agent_state.write().await;
    let current_seq = app.driver.state_seq.load(std::sync::atomic::Ordering::Acquire);
    if current_seq != expected_seq {
        return;
    }

    if let AgentState::Prompt { ref mut prompt } = *agent {
        if matches!(prompt.kind, PromptKind::Permission | PromptKind::Plan) {
            prompt.options = vec!["Accept".to_string(), "Cancel".to_string()];
            prompt.options_fallback = true;
            prompt.ready = true;

            let next = agent.clone();
            drop(agent);

            let last_message = app.driver.last_message.read().await.clone();
            let _ = app.channels.state_tx.send(TransitionEvent {
                prev: next.clone(),
                next,
                seq: expected_seq,
                cause: "ready".to_owned(),
                last_message,
            });
        }
    }
}

/// Spawn auto-dismiss of a disruption prompt in groom=auto mode.
///
/// The prompt state is broadcast BEFORE auto-dismiss so API clients see
/// the action transparently.
pub(super) fn spawn_auto_dismiss(
    store: &Arc<Store>,
    prompt: &crate::driver::PromptContext,
    config: &Config,
    state_seq: u64,
) {
    if store.config.groom != GroomLevel::Auto {
        return;
    }
    let Some(option) = disruption_option(prompt) else {
        return;
    };
    if prompt.subtype.as_deref() == Some("settings_error") {
        warn!("auto-dismissing settings error dialog (option {option})");
    }
    let Some(ref encoder) = store.config.respond_encoder else {
        return;
    };

    // "Press Enter to continue" screens have no numbered
    // options — just send Enter instead of "N" + delay + Enter.
    let steps = if prompt.options.is_empty() {
        vec![NudgeStep { bytes: b"\r".to_vec(), delay_after: None }]
    } else if prompt.kind == PromptKind::Permission {
        encoder.encode_permission(option)
    } else {
        encoder.encode_setup(option)
    };

    let store = Arc::clone(store);
    let prompt_type = prompt.kind.as_str().to_owned();
    let prompt_subtype = prompt.subtype.clone();
    let groom_option = if prompt.options.is_empty() { None } else { Some(option) };
    let dismiss_delay = config.groom_dismiss_delay();

    tokio::spawn(auto_dismiss(
        store,
        steps,
        dismiss_delay,
        state_seq,
        prompt_type,
        prompt_subtype,
        groom_option,
    ));
}

async fn auto_dismiss(
    store: Arc<Store>,
    steps: Vec<NudgeStep>,
    dismiss_delay: Duration,
    expected_seq: u64,
    prompt_type: String,
    prompt_subtype: Option<String>,
    groom_option: Option<u32>,
) {
    tokio::time::sleep(dismiss_delay).await;

    // Guard: skip if state changed (someone already responded).
    let current = store.driver.state_seq.load(std::sync::atomic::Ordering::Acquire);
    if current != expected_seq {
        return;
    }
    let _delivery = store.input_gate.acquire().await;
    // Re-check after gate acquisition.
    let current = store.driver.state_seq.load(std::sync::atomic::Ordering::Acquire);
    if current != expected_seq {
        return;
    }

    // Deliver steps one at a time. Between steps where a
    // delay is present, check whether the screen already
    // transitioned (e.g. number key auto-confirmed in a
    // picker dialog). Skip remaining steps to prevent
    // keystrokes bleeding into the next screen.
    let step_count = steps.len();
    for (i, step) in steps.into_iter().enumerate() {
        // Snapshot screen before steps with a delay when
        // more steps follow — we compare after the delay.
        let pre_lines = if step.delay_after.is_some() && i + 1 < step_count {
            store.terminal.screen.try_read().ok().map(|s| s.snapshot().lines)
        } else {
            None
        };

        if store.channels.input_tx.send(InputEvent::Write(Bytes::from(step.bytes))).await.is_err() {
            break;
        }
        if let Some(delay) = step.delay_after {
            let (drain_tx, drain_rx) = tokio::sync::oneshot::channel();
            let _ = store.channels.input_tx.send(InputEvent::WaitForDrain(drain_tx)).await;
            let _ = drain_rx.await;
            tokio::time::sleep(delay).await;
        }

        // If the screen content changed after the delay,
        // the keystroke already took full effect — stop
        // before sending the next step (e.g. Enter).
        if let Some(pre) = pre_lines {
            if let Ok(screen) = store.terminal.screen.try_read() {
                if screen.snapshot().lines != pre {
                    break;
                }
            }
        }
    }
    let _ = store.channels.prompt_tx.send(PromptOutcome {
        source: "groom".to_owned(),
        r#type: prompt_type,
        subtype: prompt_subtype,
        option: groom_option,
    });
}
