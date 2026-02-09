// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! End-to-end integration tests exercising the Claude driver against claudeless,
//! a scenario-driven Claude CLI simulator.
//!
//! Requires `claudeless` in PATH (install via `brew install alfredjeanlab/tap/claudeless`).

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use bytes::Bytes;
use coop::config::Config;
use coop::driver::AgentState;
use coop::event::{InputEvent, StateChangeEvent};
use coop::run;
use tokio::sync::broadcast;

/// Panics if `claudeless` is not installed.
fn expect_claudeless() {
    let ok = std::process::Command::new("claudeless")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(
        ok,
        "claudeless not found in PATH — install via: brew install alfredjeanlab/tap/claudeless"
    );
}

fn scenario_path(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/scenarios")
        .join(name)
        .display()
        .to_string()
}

fn claude_config(scenario: &str, prompt: &str) -> Config {
    Config {
        agent: "claude".into(),
        command: vec![
            "claudeless".into(),
            "--scenario".into(),
            scenario_path(scenario),
            prompt.into(),
        ],
        ..Config::test()
    }
}

/// Wait for a state transition matching `pred`, with a 30s timeout.
async fn wait_for(
    rx: &mut broadcast::Receiver<StateChangeEvent>,
    pred: impl Fn(&AgentState) -> bool,
) -> anyhow::Result<StateChangeEvent> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match rx.recv().await {
                Ok(e) if pred(&e.next) => return Ok(e),
                Ok(_) => continue,
                Err(e) => anyhow::bail!("state channel closed: {e}"),
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for expected state"))?
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn claude_basic_session_lifecycle() -> anyhow::Result<()> {
    expect_claudeless();

    let prepared = run::prepare(claude_config("claude_hello.toml", "hello")).await?;
    let mut rx = prepared.app_state.channels.state_tx.subscribe();
    let shutdown = prepared.app_state.lifecycle.shutdown.clone();
    let handle = tokio::spawn(prepared.run());

    wait_for(&mut rx, |s| matches!(s, AgentState::WaitingForInput)).await?;

    shutdown.cancel();
    handle.await??;

    Ok(())
}

#[tokio::test]
async fn claude_tool_use_session_lifecycle() -> anyhow::Result<()> {
    expect_claudeless();

    let prepared = run::prepare(claude_config("claude_tool_use.toml", "read a file")).await?;
    let mut rx = prepared.app_state.channels.state_tx.subscribe();
    let shutdown = prepared.app_state.lifecycle.shutdown.clone();
    let handle = tokio::spawn(prepared.run());

    wait_for(&mut rx, |s| matches!(s, AgentState::Working)).await?;
    wait_for(&mut rx, |s| matches!(s, AgentState::WaitingForInput)).await?;

    shutdown.cancel();
    handle.await??;

    Ok(())
}

#[tokio::test]
async fn claude_ask_user_session_lifecycle() -> anyhow::Result<()> {
    expect_claudeless();

    let prepared = run::prepare(claude_config("claude_ask_user.toml", "help me choose")).await?;
    let mut rx = prepared.app_state.channels.state_tx.subscribe();
    let shutdown = prepared.app_state.lifecycle.shutdown.clone();
    let input_tx = prepared.app_state.channels.input_tx.clone();
    let handle = tokio::spawn(prepared.run());

    wait_for(&mut rx, |s| matches!(s, AgentState::AskUser { .. })).await?;

    // Emulate user selecting first option in the elicitation dialog.
    tokio::time::sleep(Duration::from_millis(200)).await;
    input_tx
        .send(InputEvent::Write(Bytes::from_static(b"1")))
        .await?;

    wait_for(&mut rx, |s| matches!(s, AgentState::WaitingForInput)).await?;

    shutdown.cancel();
    handle.await??;

    Ok(())
}

#[tokio::test]
async fn claude_multi_question_session_lifecycle() -> anyhow::Result<()> {
    expect_claudeless();

    let prepared =
        run::prepare(claude_config("claude_multi_question.toml", "help me decide")).await?;
    let mut rx = prepared.app_state.channels.state_tx.subscribe();
    let shutdown = prepared.app_state.lifecycle.shutdown.clone();
    let input_tx = prepared.app_state.channels.input_tx.clone();
    let handle = tokio::spawn(prepared.run());

    // Multi-question is a single dialog with tabs: Q1 → Q2 → Confirm.
    wait_for(&mut rx, |s| matches!(s, AgentState::AskUser { .. })).await?;


    // Answer first question (select option 1).
    tokio::time::sleep(Duration::from_millis(100)).await;
    input_tx
        .send(InputEvent::Write(Bytes::from_static(b"1")))
        .await?;

    // Answer second question (select option 2).
    tokio::time::sleep(Duration::from_millis(100)).await;
    input_tx
        .send(InputEvent::Write(Bytes::from_static(b"2")))
        .await?;

    // Confirm.
    tokio::time::sleep(Duration::from_millis(100)).await;
    input_tx
        .send(InputEvent::Write(Bytes::from_static(b"\r")))
        .await?;

    wait_for(&mut rx, |s| matches!(s, AgentState::WaitingForInput)).await?;

    shutdown.cancel();
    handle.await??;

    Ok(())
}

#[tokio::test]
async fn claude_plan_mode_session_lifecycle() -> anyhow::Result<()> {
    expect_claudeless();

    let prepared =
        run::prepare(claude_config("claude_plan_mode.toml", "plan a feature")).await?;
    let mut rx = prepared.app_state.channels.state_tx.subscribe();
    let shutdown = prepared.app_state.lifecycle.shutdown.clone();
    let input_tx = prepared.app_state.channels.input_tx.clone();
    let handle = tokio::spawn(prepared.run());

    // EnterPlanMode → Working.
    wait_for(&mut rx, |s| matches!(s, AgentState::Working)).await?;

    // ExitPlanMode → PlanPrompt (user must approve).
    wait_for(&mut rx, |s| matches!(s, AgentState::PlanPrompt { .. })).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    input_tx
        .send(InputEvent::Write(Bytes::from_static(b"1")))
        .await?;

    wait_for(&mut rx, |s| matches!(s, AgentState::WaitingForInput)).await?;

    shutdown.cancel();
    handle.await??;

    Ok(())
}
