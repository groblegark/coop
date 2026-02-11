// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Named credential profiles and automatic rotation on rate limit.
//!
//! Profiles are registered via the API and stored in memory. When the agent
//! hits a rate-limit error, the session loop calls [`ProfileState::try_auto_rotate`]
//! to pick the next available profile and produce a [`SwitchRequest`].

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::debug;

use crate::driver::AgentState;
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

/// Rotation policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileConfig {
    /// Whether to automatically rotate on rate limit errors.
    pub rotate_on_rate_limit: bool,
    /// Cooldown duration in seconds before a rate-limited profile becomes available again.
    pub cooldown_secs: u64,
    /// Maximum number of rotation switches allowed per hour (anti-flap).
    pub max_switches_per_hour: u32,
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self { rotate_on_rate_limit: true, cooldown_secs: 300, max_switches_per_hour: 20 }
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
    config: RwLock<ProfileConfig>,
    switch_history: RwLock<VecDeque<Instant>>,
    /// Dedup flag: ensures only one retry timer is pending at a time.
    retry_pending: AtomicBool,
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

impl Default for ProfileState {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfileState {
    /// Create an empty profile state with default config.
    pub fn new() -> Self {
        Self {
            profiles: RwLock::new(Vec::new()),
            config: RwLock::new(ProfileConfig::default()),
            switch_history: RwLock::new(VecDeque::new()),
            retry_pending: AtomicBool::new(false),
        }
    }

    /// Replace all profiles. The first entry becomes Active.
    pub async fn register(&self, entries: Vec<ProfileEntry>, config: Option<ProfileConfig>) {
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
        if let Some(c) = config {
            *self.config.write().await = c;
        }
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

    /// Return the current config.
    pub async fn config(&self) -> ProfileConfig {
        self.config.read().await.clone()
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
            for p in profiles.iter_mut() {
                if p.name == name {
                    p.status = ProfileStatus::Active;
                } else if matches!(p.status, ProfileStatus::Active) {
                    p.status = ProfileStatus::Available;
                }
            }
        }
        found
    }

    /// Core rotation method: check config, anti-flap, mark current as rate-limited,
    /// pick next available, and return a [`RotateOutcome`].
    pub async fn try_auto_rotate(&self) -> RotateOutcome {
        let config = self.config.read().await.clone();

        // Guard: rotation disabled.
        if !config.rotate_on_rate_limit {
            return RotateOutcome::Skipped;
        }

        let mut profiles = self.profiles.write().await;

        // Guard: need at least 2 profiles to rotate.
        if profiles.len() < 2 {
            return RotateOutcome::Skipped;
        }

        // Anti-flap: check switch rate.
        {
            let mut history = self.switch_history.write().await;
            let one_hour_ago = Instant::now() - Duration::from_secs(3600);
            while history.front().is_some_and(|t| *t < one_hour_ago) {
                history.pop_front();
            }
            if history.len() as u32 >= config.max_switches_per_hour {
                return RotateOutcome::Skipped;
            }
        }

        let now = Instant::now();
        let cooldown = Duration::from_secs(config.cooldown_secs);

        // Mark current active profile as rate-limited.
        let active_idx = profiles.iter().position(|p| matches!(p.status, ProfileStatus::Active));
        if let Some(idx) = active_idx {
            profiles[idx].status = ProfileStatus::RateLimited { cooldown_until: now + cooldown };
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
