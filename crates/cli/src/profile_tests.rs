// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::collections::HashMap;

use super::*;

fn entry(name: &str) -> ProfileEntry {
    ProfileEntry {
        name: name.to_owned(),
        credentials: HashMap::from([("API_KEY".to_owned(), format!("key-{name}"))]),
    }
}

/// Extract the SwitchRequest from a RotateOutcome::Switch, panicking otherwise.
fn unwrap_switch(outcome: RotateOutcome) -> SwitchRequest {
    match outcome {
        RotateOutcome::Switch(req) => req,
        other => panic!("expected Switch, got {other:?}"),
    }
}

#[tokio::test]
async fn register_replaces_all() -> anyhow::Result<()> {
    let state = ProfileState::new();
    state.register(vec![entry("a"), entry("b"), entry("c")], None).await;

    // First entry becomes active, rest are available.
    let list = state.list().await;
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].status, "active");
    assert_eq!(list[1].status, "available");
    assert_eq!(list[2].status, "available");
    assert_eq!(state.active_name().await.as_deref(), Some("a"));

    // Re-register replaces everything.
    state.register(vec![entry("x")], None).await;
    assert_eq!(state.list().await.len(), 1);
    assert_eq!(state.list().await[0].name, "x");
    Ok(())
}

#[tokio::test]
async fn try_auto_rotate_picks_next() -> anyhow::Result<()> {
    let state = ProfileState::new();
    state.register(vec![entry("a"), entry("b"), entry("c")], None).await;

    let req = unwrap_switch(state.try_auto_rotate().await);
    assert_eq!(req.profile.as_deref(), Some("b"));
    assert!(req.force);
    assert!(req.credentials.is_some());

    // "a" should now be rate_limited, no one is active yet (set_active not called).
    let list = state.list().await;
    assert_eq!(list[0].status, "rate_limited");
    Ok(())
}

#[tokio::test]
async fn try_auto_rotate_skips_rate_limited() -> anyhow::Result<()> {
    let state = ProfileState::new();
    state.register(vec![entry("a"), entry("b"), entry("c")], None).await;

    // Rotate once: a → rate_limited, picks b.
    let req = unwrap_switch(state.try_auto_rotate().await);
    assert_eq!(req.profile.as_deref(), Some("b"));

    // Simulate: set b as active.
    state.set_active("b").await;

    // Rotate again: b → rate_limited, should skip a (still rate_limited), pick c.
    let req = unwrap_switch(state.try_auto_rotate().await);
    assert_eq!(req.profile.as_deref(), Some("c"));
    Ok(())
}

#[tokio::test]
async fn try_auto_rotate_exhausted_when_all_limited() -> anyhow::Result<()> {
    let state = ProfileState::new();
    state.register(vec![entry("a"), entry("b")], None).await;

    // Rotate: a → rate_limited, picks b.
    let req = unwrap_switch(state.try_auto_rotate().await);
    assert!(req.profile.is_some());

    // Set b as active, then rotate: b → rate_limited, a still rate_limited → Exhausted.
    state.set_active("b").await;
    let outcome = state.try_auto_rotate().await;
    match outcome {
        RotateOutcome::Exhausted { retry_after } => {
            // retry_after should be positive (cooldown_secs defaults to 300).
            assert!(retry_after.as_secs() > 0, "retry_after should be positive: {retry_after:?}");
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn try_auto_rotate_respects_anti_flap() -> anyhow::Result<()> {
    let state = ProfileState::new();
    let config = ProfileConfig { max_switches_per_hour: 2, cooldown_secs: 0, ..Default::default() };
    state.register(vec![entry("a"), entry("b"), entry("c")], Some(config)).await;

    // Two rotations should succeed.
    let r1 = unwrap_switch(state.try_auto_rotate().await);
    state.set_active(r1.profile.as_deref().unwrap()).await;

    let r2 = unwrap_switch(state.try_auto_rotate().await);
    state.set_active(r2.profile.as_deref().unwrap()).await;

    // Third should be blocked by anti-flap.
    assert!(matches!(state.try_auto_rotate().await, RotateOutcome::Skipped));
    Ok(())
}

#[tokio::test]
async fn try_auto_rotate_disabled_by_config() -> anyhow::Result<()> {
    let state = ProfileState::new();
    let config = ProfileConfig { rotate_on_rate_limit: false, ..Default::default() };
    state.register(vec![entry("a"), entry("b")], Some(config)).await;

    assert!(matches!(state.try_auto_rotate().await, RotateOutcome::Skipped));
    Ok(())
}

#[tokio::test]
async fn try_auto_rotate_needs_at_least_two_profiles() -> anyhow::Result<()> {
    let state = ProfileState::new();
    state.register(vec![entry("a")], None).await;
    assert!(matches!(state.try_auto_rotate().await, RotateOutcome::Skipped));

    // No profiles at all.
    let empty = ProfileState::new();
    assert!(matches!(empty.try_auto_rotate().await, RotateOutcome::Skipped));
    Ok(())
}

#[tokio::test]
async fn set_active_tracks_profile() -> anyhow::Result<()> {
    let state = ProfileState::new();
    state.register(vec![entry("a"), entry("b")], None).await;

    assert_eq!(state.active_name().await.as_deref(), Some("a"));

    state.set_active("b").await;
    assert_eq!(state.active_name().await.as_deref(), Some("b"));

    // "a" should no longer be active.
    let list = state.list().await;
    assert_eq!(list[0].status, "available");
    assert_eq!(list[1].status, "active");

    // Credentials resolve correctly for both profiles.
    let creds = state.resolve_credentials("b").await;
    assert!(creds.is_some());
    assert_eq!(creds.unwrap().get("API_KEY").unwrap(), "key-b");
    assert!(state.resolve_credentials("nonexistent").await.is_none());
    Ok(())
}

#[tokio::test]
async fn retry_pending_dedup() -> anyhow::Result<()> {
    let state = ProfileState::new();
    // Initially false.
    assert!(!state.retry_pending.load(std::sync::atomic::Ordering::Acquire));

    // First swap sets it to true, returns false (was not pending).
    let was_pending = state.retry_pending.swap(true, std::sync::atomic::Ordering::AcqRel);
    assert!(!was_pending);

    // Second swap returns true (already pending) — schedule_retry would bail.
    let was_pending = state.retry_pending.swap(true, std::sync::atomic::Ordering::AcqRel);
    assert!(was_pending);

    // Clear it.
    state.retry_pending.store(false, std::sync::atomic::Ordering::Release);
    let was_pending = state.retry_pending.swap(true, std::sync::atomic::Ordering::AcqRel);
    assert!(!was_pending);
    Ok(())
}

#[tokio::test]
async fn exhausted_retry_after_uses_shortest_cooldown() -> anyhow::Result<()> {
    let state = ProfileState::new();
    // Use a short cooldown to test.
    let config = ProfileConfig { cooldown_secs: 10, ..Default::default() };
    state.register(vec![entry("a"), entry("b"), entry("c")], Some(config)).await;

    // Exhaust a → rate_limited (cooldown: 10s), picks b.
    let _r1 = unwrap_switch(state.try_auto_rotate().await);
    state.set_active("b").await;

    // Exhaust b → rate_limited (cooldown: 10s), picks c.
    let _r2 = unwrap_switch(state.try_auto_rotate().await);
    state.set_active("c").await;

    // Exhaust c → all rate_limited → Exhausted.
    let outcome = state.try_auto_rotate().await;
    match outcome {
        RotateOutcome::Exhausted { retry_after } => {
            // retry_after should be ≤ 10s (shortest remaining cooldown).
            assert!(retry_after.as_secs() <= 10, "retry_after too large: {retry_after:?}");
            assert!(retry_after.as_secs() > 0, "retry_after should be positive");
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
    Ok(())
}
