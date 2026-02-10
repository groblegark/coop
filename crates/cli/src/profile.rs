// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Named credential profiles and automatic rotation on rate limit.
//!
//! Profiles are registered via the API and stored in memory. When the agent
//! hits a rate-limit error, the session loop calls [`ProfileState::try_auto_rotate`]
//! to pick the next available profile and produce a [`SwitchRequest`].

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::switch::SwitchRequest;

/// A registered credential profile.
pub struct Profile {
    pub name: String,
    pub credentials: HashMap<String, String>,
    pub status: ProfileStatus,
}

/// Current status of a profile.
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
pub struct ProfileConfig {
    /// Whether to automatically rotate on rate limit errors.
    #[serde(default = "default_true")]
    pub rotate_on_rate_limit: bool,
    /// Cooldown duration in seconds before a rate-limited profile becomes available again.
    #[serde(default = "default_cooldown")]
    pub cooldown_secs: u64,
    /// Maximum number of rotation switches allowed per hour (anti-flap).
    #[serde(default = "default_max_switches")]
    pub max_switches_per_hour: u32,
}

fn default_true() -> bool {
    true
}
fn default_cooldown() -> u64 {
    300
}
fn default_max_switches() -> u32 {
    20
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
}

impl std::fmt::Debug for ProfileState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfileState").finish()
    }
}

/// Entry in a registration request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileEntry {
    pub name: String,
    pub credentials: HashMap<String, String>,
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

    /// Whether any profiles are registered.
    pub async fn has_profiles(&self) -> bool {
        !self.profiles.read().await.is_empty()
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
    /// pick next available, and return a SwitchRequest or None.
    pub async fn try_auto_rotate(&self) -> Option<SwitchRequest> {
        let config = self.config.read().await.clone();

        // Guard: rotation disabled.
        if !config.rotate_on_rate_limit {
            return None;
        }

        let mut profiles = self.profiles.write().await;

        // Guard: need at least 2 profiles to rotate.
        if profiles.len() < 2 {
            return None;
        }

        // Anti-flap: check switch rate.
        {
            let mut history = self.switch_history.write().await;
            let one_hour_ago = Instant::now() - std::time::Duration::from_secs(3600);
            while history.front().is_some_and(|t| *t < one_hour_ago) {
                history.pop_front();
            }
            if history.len() as u32 >= config.max_switches_per_hour {
                return None;
            }
        }

        let now = Instant::now();
        let cooldown = std::time::Duration::from_secs(config.cooldown_secs);

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

        let next_idx = next_idx?;
        let next_name = profiles[next_idx].name.clone();
        let next_creds = profiles[next_idx].credentials.clone();

        // Record switch timestamp.
        self.switch_history.write().await.push_back(Instant::now());

        Some(SwitchRequest { credentials: Some(next_creds), force: true, profile: Some(next_name) })
    }
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
