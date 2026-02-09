// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{AgentState, CompositeDetector, DetectedState, ExitStatus, PromptContext, PromptKind};
use crate::driver::grace::IdleGraceTimer;
use crate::test_support::MockDetector;

/// Helper: run a CompositeDetector with given detectors and collect emitted states.
async fn run_composite(
    detectors: Vec<Box<dyn super::Detector>>,
    grace_duration: Duration,
    activity_counter: Arc<AtomicU64>,
    collect_timeout: Duration,
) -> anyhow::Result<Vec<DetectedState>> {
    let (output_tx, mut output_rx) = mpsc::channel(64);
    let grace_timer = IdleGraceTimer::new(grace_duration);
    let composite = CompositeDetector {
        tiers: detectors,
        grace_timer,
        grace_tick_interval: Duration::from_secs(1),
    };

    let activity_fn: Arc<dyn Fn() -> u64 + Send + Sync> = {
        let counter = Arc::clone(&activity_counter);
        Arc::new(move || counter.load(Ordering::Relaxed))
    };
    let grace_deadline = Arc::new(parking_lot::Mutex::new(None));
    let shutdown = CancellationToken::new();

    let sd = shutdown.clone();
    tokio::spawn(async move {
        composite
            .run(output_tx, activity_fn, grace_deadline, sd)
            .await;
    });

    let mut results = Vec::new();
    let deadline = tokio::time::Instant::now() + collect_timeout;

    loop {
        tokio::select! {
            state = output_rx.recv() => {
                match state {
                    Some(s) => results.push(s),
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    shutdown.cancel();
    Ok(results)
}

#[tokio::test]
async fn higher_confidence_wins() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn super::Detector>> = vec![
        Box::new(MockDetector::new(
            1,
            vec![(Duration::from_millis(50), AgentState::Working)],
        )),
        Box::new(MockDetector::new(
            3,
            vec![(Duration::from_millis(100), AgentState::WaitingForInput)],
        )),
    ];

    let results = run_composite(
        detectors,
        Duration::from_secs(60),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(500),
    )
    .await?;

    assert!(!results.is_empty(), "expected at least one state emission");
    assert_eq!(results[0].state, AgentState::Working);
    assert_eq!(results[0].tier, 1);

    let has_waiting = results
        .iter()
        .any(|s| s.state == AgentState::WaitingForInput);
    assert!(
        !has_waiting,
        "WaitingForInput from lower tier should be gated by grace"
    );
    Ok(())
}

#[tokio::test]
async fn lower_confidence_accepted_immediately_for_non_idle() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn super::Detector>> = vec![
        Box::new(MockDetector::new(1, vec![])),
        Box::new(MockDetector::new(
            3,
            vec![(Duration::from_millis(50), AgentState::Working)],
        )),
    ];

    let results = run_composite(
        detectors,
        Duration::from_secs(60),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(300),
    )
    .await?;

    assert!(!results.is_empty(), "expected Working from tier 3");
    assert_eq!(results[0].state, AgentState::Working);
    assert_eq!(results[0].tier, 3);
    Ok(())
}

#[tokio::test]
async fn lower_confidence_idle_triggers_grace() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn super::Detector>> = vec![
        Box::new(MockDetector::new(
            1,
            vec![(Duration::from_millis(50), AgentState::Working)],
        )),
        Box::new(MockDetector::new(
            3,
            vec![(Duration::from_millis(100), AgentState::WaitingForInput)],
        )),
    ];

    let results = run_composite(
        detectors,
        Duration::from_secs(2),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(500),
    )
    .await?;

    let working = results.iter().any(|s| s.state == AgentState::Working);
    assert!(working, "expected Working state");

    let waiting = results
        .iter()
        .any(|s| s.state == AgentState::WaitingForInput);
    assert!(!waiting, "WaitingForInput should be held by grace timer");
    Ok(())
}

#[tokio::test]
async fn grace_cancelled_by_activity() -> anyhow::Result<()> {
    let activity = Arc::new(AtomicU64::new(0));
    let activity_writer = Arc::clone(&activity);

    let detectors: Vec<Box<dyn super::Detector>> = vec![
        Box::new(MockDetector::new(
            1,
            vec![(Duration::from_millis(50), AgentState::Working)],
        )),
        Box::new(MockDetector::new(
            3,
            vec![(Duration::from_millis(150), AgentState::WaitingForInput)],
        )),
    ];

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(250)).await;
        activity_writer.store(100, Ordering::Relaxed);
    });

    let results = run_composite(
        detectors,
        Duration::from_secs(2),
        activity,
        Duration::from_secs(4),
    )
    .await?;

    let waiting = results
        .iter()
        .any(|s| s.state == AgentState::WaitingForInput);
    assert!(!waiting, "WaitingForInput should be cancelled by activity");
    Ok(())
}

#[tokio::test]
async fn equal_tier_replaces_state() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn super::Detector>> = vec![Box::new(MockDetector::new(
        2,
        vec![
            (Duration::from_millis(50), AgentState::Working),
            (Duration::from_millis(100), AgentState::WaitingForInput),
        ],
    ))];

    let results = run_composite(
        detectors,
        Duration::from_secs(60),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(300),
    )
    .await?;

    assert!(
        results.len() >= 2,
        "expected at least 2 states: {results:?}"
    );
    assert_eq!(results[0].state, AgentState::Working);
    assert_eq!(results[1].state, AgentState::WaitingForInput);
    Ok(())
}

#[tokio::test]
async fn terminal_state_always_accepted() -> anyhow::Result<()> {
    let exit = AgentState::Exited {
        status: ExitStatus {
            code: Some(0),
            signal: None,
        },
    };

    let detectors: Vec<Box<dyn super::Detector>> = vec![
        Box::new(MockDetector::new(
            1,
            vec![(Duration::from_millis(50), AgentState::Working)],
        )),
        Box::new(MockDetector::new(
            3,
            vec![(Duration::from_millis(100), exit.clone())],
        )),
    ];

    let results = run_composite(
        detectors,
        Duration::from_secs(60),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(300),
    )
    .await?;

    let has_exited = results
        .iter()
        .any(|s| matches!(s.state, AgentState::Exited { .. }));
    assert!(
        has_exited,
        "terminal state should be accepted from any tier"
    );
    Ok(())
}

#[tokio::test]
async fn dedup_suppresses_identical() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn super::Detector>> = vec![Box::new(MockDetector::new(
        1,
        vec![
            (Duration::from_millis(50), AgentState::Working),
            (Duration::from_millis(100), AgentState::Working),
        ],
    ))];

    let results = run_composite(
        detectors,
        Duration::from_secs(60),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(300),
    )
    .await?;

    let working_count = results
        .iter()
        .filter(|s| s.state == AgentState::Working)
        .count();
    assert_eq!(
        working_count, 1,
        "duplicate state should be suppressed: {results:?}"
    );
    Ok(())
}

fn empty_prompt(kind: PromptKind) -> PromptContext {
    PromptContext {
        kind,
        tool: None,
        input_preview: None,
        screen_lines: vec![],
        questions: vec![],
        question_current: 0,
    }
}

#[tokio::test]
async fn tier1_supersedes_tier5_screen_idle() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn super::Detector>> = vec![
        Box::new(MockDetector::new(
            1,
            vec![(Duration::from_millis(50), AgentState::Working)],
        )),
        Box::new(MockDetector::new(
            5,
            vec![(Duration::from_millis(100), AgentState::WaitingForInput)],
        )),
    ];

    let results = run_composite(
        detectors,
        Duration::from_secs(60),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(500),
    )
    .await?;

    assert!(!results.is_empty(), "expected at least one state emission");
    assert_eq!(results[0].state, AgentState::Working);
    assert_eq!(results[0].tier, 1);

    let has_waiting = results
        .iter()
        .any(|s| s.state == AgentState::WaitingForInput);
    assert!(
        !has_waiting,
        "tier 5 WaitingForInput should be gated by grace when tier 1 is active"
    );
    Ok(())
}

#[tokio::test]
async fn tier2_supersedes_tier5_screen_idle() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn super::Detector>> = vec![
        Box::new(MockDetector::new(
            2,
            vec![(Duration::from_millis(50), AgentState::Working)],
        )),
        Box::new(MockDetector::new(
            5,
            vec![(Duration::from_millis(100), AgentState::WaitingForInput)],
        )),
    ];

    let results = run_composite(
        detectors,
        Duration::from_secs(60),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(500),
    )
    .await?;

    assert!(!results.is_empty(), "expected at least one state emission");
    assert_eq!(results[0].state, AgentState::Working);
    assert_eq!(results[0].tier, 2);

    let has_waiting = results
        .iter()
        .any(|s| s.state == AgentState::WaitingForInput);
    assert!(
        !has_waiting,
        "tier 5 WaitingForInput should be gated by grace when tier 2 is active"
    );
    Ok(())
}

/// Regression: Claude fires both `PreToolUse(ExitPlanMode)` → Prompt(Plan) and
/// `Notification(permission_prompt)` → Prompt(Permission) for the same user-facing
/// plan approval moment. When the permission notification arrives after the
/// PreToolUse event, the composite detector must not let the generic
/// Permission prompt overwrite the more specific Plan prompt.
#[tokio::test]
async fn plan_prompt_not_overwritten_by_permission_prompt() -> anyhow::Result<()> {
    // Simulate tier 1 emitting Plan prompt then Permission prompt in quick succession.
    let detectors: Vec<Box<dyn super::Detector>> = vec![Box::new(MockDetector::new(
        1,
        vec![
            (
                Duration::from_millis(50),
                AgentState::Prompt {
                    prompt: empty_prompt(PromptKind::Plan),
                },
            ),
            (
                Duration::from_millis(10),
                AgentState::Prompt {
                    prompt: empty_prompt(PromptKind::Permission),
                },
            ),
        ],
    ))];

    let results = run_composite(
        detectors,
        Duration::from_secs(60),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(300),
    )
    .await?;

    // The final settled state should be Plan prompt, not Permission prompt.
    let last = results
        .last()
        .expect("expected at least one state emission");
    assert!(
        matches!(
            last.state,
            AgentState::Prompt {
                prompt: PromptContext {
                    kind: PromptKind::Plan,
                    ..
                }
            }
        ),
        "expected final state to be Plan prompt, got {:?}",
        last.state,
    );
    Ok(())
}
