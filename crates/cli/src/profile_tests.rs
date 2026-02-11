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

    let req = state.try_auto_rotate().await;
    assert!(req.is_some());
    let req = req.unwrap();
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
    let req = state.try_auto_rotate().await;
    assert_eq!(req.as_ref().and_then(|r| r.profile.as_deref()), Some("b"));

    // Simulate: set b as active.
    state.set_active("b").await;

    // Rotate again: b → rate_limited, should skip a (still rate_limited), pick c.
    let req = state.try_auto_rotate().await;
    assert_eq!(req.as_ref().and_then(|r| r.profile.as_deref()), Some("c"));
    Ok(())
}

#[tokio::test]
async fn try_auto_rotate_none_when_all_limited() -> anyhow::Result<()> {
    let state = ProfileState::new();
    state.register(vec![entry("a"), entry("b")], None).await;

    // Rotate: a → rate_limited, picks b.
    let req = state.try_auto_rotate().await;
    assert!(req.is_some());

    // Set b as active, then rotate: b → rate_limited, a still rate_limited → None.
    state.set_active("b").await;
    let req = state.try_auto_rotate().await;
    assert!(req.is_none());
    Ok(())
}

#[tokio::test]
async fn try_auto_rotate_respects_anti_flap() -> anyhow::Result<()> {
    let state = ProfileState::new();
    let config = ProfileConfig { max_switches_per_hour: 2, cooldown_secs: 0, ..Default::default() };
    state.register(vec![entry("a"), entry("b"), entry("c")], Some(config)).await;

    // Two rotations should succeed.
    let r1 = state.try_auto_rotate().await;
    assert!(r1.is_some());
    state.set_active(r1.as_ref().unwrap().profile.as_deref().unwrap()).await;

    let r2 = state.try_auto_rotate().await;
    assert!(r2.is_some());
    state.set_active(r2.as_ref().unwrap().profile.as_deref().unwrap()).await;

    // Third should be blocked by anti-flap.
    let r3 = state.try_auto_rotate().await;
    assert!(r3.is_none());
    Ok(())
}

#[tokio::test]
async fn try_auto_rotate_disabled_by_config() -> anyhow::Result<()> {
    let state = ProfileState::new();
    let config = ProfileConfig { rotate_on_rate_limit: false, ..Default::default() };
    state.register(vec![entry("a"), entry("b")], Some(config)).await;

    assert!(state.try_auto_rotate().await.is_none());
    Ok(())
}

#[tokio::test]
async fn try_auto_rotate_needs_at_least_two_profiles() -> anyhow::Result<()> {
    let state = ProfileState::new();
    state.register(vec![entry("a")], None).await;
    assert!(state.try_auto_rotate().await.is_none());

    // No profiles at all.
    let empty = ProfileState::new();
    assert!(empty.try_auto_rotate().await.is_none());
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
