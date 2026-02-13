// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Named credential profiles and automatic rotation on rate limit.
//!
//! Profiles are registered via the API and stored in memory. When the agent
//! hits a rate-limit error, the session loop calls [`ProfileState::try_auto_rotate`]
//! to pick the next available profile and produce a [`SwitchRequest`].

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tracing::debug;

use crate::driver::AgentState;
use crate::event::ProfileEvent;
use crate::switch::SwitchRequest;

/// A registered credential profile.
#[derive(Debug)]
pub struct Profile {
    pub name: String,
    pub credentials: HashMap<String, String>,
    pub status: ProfileStatus,
}

/// Current status of a profile.
#[derive(Debug)]
pub enum ProfileStatus {
    /// This profile is currently in use.
    Active,
    /// This profile is available for rotation.
    Available,
    /// This profile hit a rate limit and is cooling down.
    RateLimited { cooldown_until: Instant },
}

/// Process-wide profile rotation mode.
///
/// - `Auto`: automatically rotate on rate limit errors.
/// - `Manual`: detection works but no automatic rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileMode {
    Auto,
    Manual,
}

impl ProfileMode {
    fn as_u8(self) -> u8 {
        match self {
            Self::Auto => 0,
            Self::Manual => 1,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Manual,
            _ => Self::Auto,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

impl std::fmt::Display for ProfileMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ProfileMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "manual" => Ok(Self::Manual),
            other => anyhow::bail!("invalid profile mode: {other} (expected auto or manual)"),
        }
    }
}

/// Serializable snapshot of a profile's state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileInfo {
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_remaining_secs: Option<u64>,
}

/// Shared profile state. Lives on `Store`.
pub struct ProfileState {
    profiles: RwLock<Vec<Profile>>,
    /// Process-wide rotation mode (0=auto, 1=manual).
    mode: AtomicU8,
    switch_history: RwLock<VecDeque<Instant>>,
    /// Dedup flag: ensures only one retry timer is pending at a time.
    retry_pending: AtomicBool,
    /// Broadcast channel for profile lifecycle events.
    pub profile_tx: broadcast::Sender<ProfileEvent>,
}

/// Entry in a registration request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileEntry {
    pub name: String,
    pub credentials: HashMap<String, String>,
}

/// Result of attempting automatic profile rotation.
#[derive(Debug)]
pub enum RotateOutcome {
    /// Switch to this profile now.
    Switch(SwitchRequest),
    /// All profiles on cooldown; retry after this duration.
    Exhausted { retry_after: Duration },
    /// Rotation not applicable (disabled, < 2 profiles, anti-flap).
    Skipped,
}

/// Read a `u64` from an env var, falling back to a default.
fn env_u64(var: &str, default: u64) -> u64 {
    std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Read a `u32` from an env var, falling back to a default.
fn env_u32(var: &str, default: u32) -> u32 {
    std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

impl Default for ProfileState {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfileState {
    /// Create an empty profile state with default config.
    pub fn new() -> Self {
        let (profile_tx, _) = broadcast::channel(64);
        Self {
            profiles: RwLock::new(Vec::new()),
            mode: AtomicU8::new(ProfileMode::Auto.as_u8()),
            switch_history: RwLock::new(VecDeque::new()),
            retry_pending: AtomicBool::new(false),
            profile_tx,
        }
    }

    /// Return the current rotation mode.
    pub fn mode(&self) -> ProfileMode {
        ProfileMode::from_u8(self.mode.load(Ordering::Acquire))
    }

    /// Set the rotation mode.
    pub fn set_mode(&self, mode: ProfileMode) {
        self.mode.store(mode.as_u8(), Ordering::Release);
    }

    /// Replace all profiles. The first entry becomes Active.
    pub async fn register(&self, entries: Vec<ProfileEntry>) {
        let mut profiles = self.profiles.write().await;
        *profiles = entries
            .into_iter()
            .enumerate()
            .map(|(i, e)| Profile {
                name: e.name,
                credentials: e.credentials,
                status: if i == 0 { ProfileStatus::Active } else { ProfileStatus::Available },
            })
            .collect();
    }

    /// Return a serializable snapshot of all profiles.
    pub async fn list(&self) -> Vec<ProfileInfo> {
        let profiles = self.profiles.read().await;
        let now = Instant::now();
        profiles
            .iter()
            .map(|p| {
                let (status, cooldown) = match &p.status {
                    ProfileStatus::Active => ("active".to_owned(), None),
                    ProfileStatus::Available => ("available".to_owned(), None),
                    ProfileStatus::RateLimited { cooldown_until } => {
                        let remaining = cooldown_until.saturating_duration_since(now).as_secs();
                        ("rate_limited".to_owned(), Some(remaining))
                    }
                };
                ProfileInfo { name: p.name.clone(), status, cooldown_remaining_secs: cooldown }
            })
            .collect()
    }

    /// Return the name of the currently active profile, if any.
    pub async fn active_name(&self) -> Option<String> {
        let profiles = self.profiles.read().await;
        profiles.iter().find(|p| matches!(p.status, ProfileStatus::Active)).map(|p| p.name.clone())
    }

    /// Resolve credentials for a named profile.
    pub async fn resolve_credentials(&self, name: &str) -> Option<HashMap<String, String>> {
        let profiles = self.profiles.read().await;
        profiles.iter().find(|p| p.name == name).map(|p| p.credentials.clone())
    }

    /// Mark a profile as Active after a successful switch.
    pub async fn set_active(&self, name: &str) -> bool {
        let mut profiles = self.profiles.write().await;
        let found = profiles.iter().any(|p| p.name == name);
        if found {
            let prev_active = profiles
                .iter()
                .find(|p| matches!(p.status, ProfileStatus::Active))
                .map(|p| p.name.clone());
            for p in profiles.iter_mut() {
                if p.name == name {
                    p.status = ProfileStatus::Active;
                } else if matches!(p.status, ProfileStatus::Active) {
                    p.status = ProfileStatus::Available;
                }
            }
            drop(profiles);
            let _ = self
                .profile_tx
                .send(ProfileEvent::ProfileSwitched { from: prev_active, to: name.to_owned() });
        }
        found
    }

    /// Core rotation method: check mode, anti-flap, mark current as rate-limited,
    /// pick next available, and return a [`RotateOutcome`].
    pub async fn try_auto_rotate(&self) -> RotateOutcome {
        // Guard: rotation disabled.
        if self.mode() == ProfileMode::Manual {
            return RotateOutcome::Skipped;
        }

        let mut profiles = self.profiles.write().await;

        // Guard: need at least 2 profiles to rotate.
        if profiles.len() < 2 {
            return RotateOutcome::Skipped;
        }

        // Anti-flap: check switch rate.
        let max_switches_per_hour = env_u32("COOP_ROTATE_MAX_PER_HOUR", 20);
        {
            let mut history = self.switch_history.write().await;
            let one_hour_ago = Instant::now() - Duration::from_secs(3600);
            while history.front().is_some_and(|t| *t < one_hour_ago) {
                history.pop_front();
            }
            if history.len() as u32 >= max_switches_per_hour {
                return RotateOutcome::Skipped;
            }
        }

        let now = Instant::now();
        let cooldown_secs = env_u64("COOP_ROTATE_COOLDOWN_SECS", 300);
        let cooldown = Duration::from_secs(cooldown_secs);

        // Mark current active profile as rate-limited.
        let active_idx = profiles.iter().position(|p| matches!(p.status, ProfileStatus::Active));
        if let Some(idx) = active_idx {
            let exhausted_name = profiles[idx].name.clone();
            profiles[idx].status = ProfileStatus::RateLimited { cooldown_until: now + cooldown };
            let _ =
                self.profile_tx.send(ProfileEvent::ProfileExhausted { profile: exhausted_name });
        }

        // Promote expired cooldowns to Available.
        for p in profiles.iter_mut() {
            if let ProfileStatus::RateLimited { cooldown_until } = &p.status {
                if *cooldown_until <= now {
                    p.status = ProfileStatus::Available;
                }
            }
        }

        // Find next Available profile (round-robin from after active).
        let start = active_idx.map(|i| i + 1).unwrap_or(0);
        let len = profiles.len();
        let next_idx = (0..len)
            .map(|offset| (start + offset) % len)
            .find(|&i| matches!(profiles[i].status, ProfileStatus::Available));

        match next_idx {
            Some(idx) => {
                let next_name = profiles[idx].name.clone();
                let next_creds = profiles[idx].credentials.clone();

                // Record switch timestamp.
                // Drop profiles lock before acquiring switch_history to avoid
                // lock-order issues (both are RwLocks on the same struct).
                drop(profiles);
                self.switch_history.write().await.push_back(Instant::now());

                RotateOutcome::Switch(SwitchRequest {
                    credentials: Some(next_creds),
                    force: true,
                    profile: Some(next_name),
                })
            }
            None => {
                // All profiles on cooldown — compute retry_after from the
                // shortest remaining cooldown.
                let retry_after = profiles
                    .iter()
                    .filter_map(|p| match &p.status {
                        ProfileStatus::RateLimited { cooldown_until } => {
                            Some(cooldown_until.saturating_duration_since(now))
                        }
                        _ => None,
                    })
                    .min()
                    .unwrap_or(cooldown);
                let _ = self.profile_tx.send(ProfileEvent::ProfileRotationExhausted {
                    retry_after_secs: retry_after.as_secs(),
                });
                RotateOutcome::Exhausted { retry_after }
            }
        }
    }

    /// Spawn a delayed retry task that calls `try_auto_rotate` once cooldowns expire.
    ///
    /// Uses an `AtomicBool` flag to ensure only one retry timer is pending.
    /// The timer no-ops if the agent is no longer in `Parked` state when it fires.
    pub fn schedule_retry(
        self: &Arc<Self>,
        retry_after: Duration,
        store: Arc<crate::transport::Store>,
    ) {
        // Dedup: only one retry timer at a time.
        if self.retry_pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let profile = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(retry_after).await;

            // Clear the dedup flag so future retries can schedule.
            profile.retry_pending.store(false, Ordering::Release);

            // Guard: only retry if the agent is still Parked.
            let current = store.driver.agent_state.read().await;
            if !matches!(&*current, AgentState::Parked { .. }) {
                debug!("retry timer fired but agent is no longer parked, skipping");
                return;
            }
            drop(current);

            match profile.try_auto_rotate().await {
                RotateOutcome::Switch(req) => {
                    debug!("retry timer: cooldown expired, switching to profile {:?}", req.profile);
                    let _ = store.switch.switch_tx.try_send(req);
                }
                RotateOutcome::Exhausted { retry_after } => {
                    debug!("retry timer: still exhausted, re-scheduling in {retry_after:?}");
                    profile.schedule_retry(retry_after, store);
                }
                RotateOutcome::Skipped => {
                    debug!("retry timer: rotation skipped");
                }
            }
        });
    }
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
