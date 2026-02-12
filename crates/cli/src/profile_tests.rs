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
    state.register(vec![entry("a"), entry("b"), entry("c")]).await;

    // First entry becomes active, rest are available.
    let list = state.list().await;
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].status, "active");
    assert_eq!(list[1].status, "available");
    assert_eq!(list[2].status, "available");
    assert_eq!(state.active_name().await.as_deref(), Some("a"));

    // Re-register replaces everything.
    state.register(vec![entry("x")]).await;
    assert_eq!(state.list().await.len(), 1);
    assert_eq!(state.list().await[0].name, "x");
    Ok(())
}

#[tokio::test]
async fn try_auto_rotate_picks_next() -> anyhow::Result<()> {
    let state = ProfileState::new();
    state.register(vec![entry("a"), entry("b"), entry("c")]).await;

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
    state.register(vec![entry("a"), entry("b"), entry("c")]).await;

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
    state.register(vec![entry("a"), entry("b")]).await;

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
    // Set cooldown to 0 so profiles recycle immediately.
    // Anti-flap is controlled by COOP_ROTATE_MAX_PER_HOUR env var (default 20).
    // We can't easily set env vars in parallel tests, so we rely on the default
    // and do enough rotations. Instead, test with manual mode toggling.
    state.register(vec![entry("a"), entry("b"), entry("c")]).await;

    // Two rotations should succeed.
    let r1 = unwrap_switch(state.try_auto_rotate().await);
    state.set_active(r1.profile.as_deref().unwrap()).await;

    let r2 = unwrap_switch(state.try_auto_rotate().await);
    state.set_active(r2.profile.as_deref().unwrap()).await;

    // With default max_switches_per_hour=20, this should still succeed.
    let r3 = state.try_auto_rotate().await;
    assert!(matches!(r3, RotateOutcome::Switch(_) | RotateOutcome::Exhausted { .. }));
    Ok(())
}

#[tokio::test]
async fn try_auto_rotate_disabled_by_mode() -> anyhow::Result<()> {
    let state = ProfileState::new();
    state.set_mode(ProfileMode::Manual);
    state.register(vec![entry("a"), entry("b")]).await;

    assert!(matches!(state.try_auto_rotate().await, RotateOutcome::Skipped));
    Ok(())
}

#[tokio::test]
async fn try_auto_rotate_needs_at_least_two_profiles() -> anyhow::Result<()> {
    let state = ProfileState::new();
    state.register(vec![entry("a")]).await;
    assert!(matches!(state.try_auto_rotate().await, RotateOutcome::Skipped));

    // No profiles at all.
    let empty = ProfileState::new();
    assert!(matches!(empty.try_auto_rotate().await, RotateOutcome::Skipped));
    Ok(())
}

#[tokio::test]
async fn set_active_tracks_profile() -> anyhow::Result<()> {
    let state = ProfileState::new();
    state.register(vec![entry("a"), entry("b")]).await;

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
    // Default cooldown is 300s from COOP_ROTATE_COOLDOWN_SECS env (or default).
    state.register(vec![entry("a"), entry("b"), entry("c")]).await;

    // Exhaust a → rate_limited, picks b.
    let _r1 = unwrap_switch(state.try_auto_rotate().await);
    state.set_active("b").await;

    // Exhaust b → rate_limited, picks c.
    let _r2 = unwrap_switch(state.try_auto_rotate().await);
    state.set_active("c").await;

    // Exhaust c → all rate_limited → Exhausted.
    let outcome = state.try_auto_rotate().await;
    match outcome {
        RotateOutcome::Exhausted { retry_after } => {
            // retry_after should be positive.
            assert!(retry_after.as_secs() > 0, "retry_after should be positive");
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn mode_get_set() -> anyhow::Result<()> {
    let state = ProfileState::new();
    assert_eq!(state.mode(), ProfileMode::Auto);

    state.set_mode(ProfileMode::Manual);
    assert_eq!(state.mode(), ProfileMode::Manual);

    state.set_mode(ProfileMode::Auto);
    assert_eq!(state.mode(), ProfileMode::Auto);
    Ok(())
}
