// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Agent pod registry (Epic 16b).
//!
//! Agent pods register with the broker on startup. The broker tracks which
//! pods are alive and what credential profiles they need. Health checks
//! prune dead pods.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// How often the broker health-checks registered pods.
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// How long before a pod that fails health checks is pruned.
const PRUNE_AFTER: Duration = Duration::from_secs(300);

/// A registered agent pod.
#[derive(Debug, Clone)]
pub struct RegisteredPod {
    /// Human-readable pod name (e.g. "mayor", "dev-1").
    pub name: String,
    /// Coop HTTP API base URL (e.g. "http://10.42.0.15:3000").
    pub coop_url: String,
    /// Which credential profiles this pod needs (empty = all).
    pub profiles_needed: Vec<String>,
    /// Last time this pod was seen alive (registration or health check).
    pub last_seen: Instant,
    /// Whether the last health check succeeded.
    pub healthy: bool,
    /// Optional auth token for the pod's coop API.
    pub auth_token: Option<String>,
}

/// Serializable snapshot for the status API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodInfo {
    pub name: String,
    pub coop_url: String,
    pub profiles_needed: Vec<String>,
    pub healthy: bool,
    pub last_seen_secs_ago: u64,
}

/// Request body for pod registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    /// Pod name (must be unique).
    pub name: String,
    /// Coop HTTP API base URL.
    pub coop_url: String,
    /// Which credential profiles this pod needs (empty = all).
    #[serde(default)]
    pub profiles_needed: Vec<String>,
    /// Auth token for the pod's coop API.
    #[serde(default)]
    pub auth_token: Option<String>,
}

/// Pod registry — tracks all agent pods the broker knows about.
pub struct PodRegistry {
    pods: RwLock<HashMap<String, RegisteredPod>>,
    http_client: reqwest::Client,
}

impl Default for PodRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PodRegistry {
    pub fn new() -> Self {
        Self {
            pods: RwLock::new(HashMap::new()),
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// Register or update a pod. Returns true if this is a new registration.
    pub async fn register(&self, req: RegisterRequest) -> bool {
        let mut pods = self.pods.write().await;
        let is_new = !pods.contains_key(&req.name);

        pods.insert(
            req.name.clone(),
            RegisteredPod {
                name: req.name,
                coop_url: req.coop_url,
                profiles_needed: req.profiles_needed,
                last_seen: Instant::now(),
                healthy: true,
                auth_token: req.auth_token,
            },
        );

        is_new
    }

    /// Remove a pod by name.
    pub async fn deregister(&self, name: &str) -> bool {
        self.pods.write().await.remove(name).is_some()
    }

    /// Return a snapshot of all registered pods.
    pub async fn list(&self) -> Vec<PodInfo> {
        let pods = self.pods.read().await;
        let now = Instant::now();
        pods.values()
            .map(|p| PodInfo {
                name: p.name.clone(),
                coop_url: p.coop_url.clone(),
                profiles_needed: p.profiles_needed.clone(),
                healthy: p.healthy,
                last_seen_secs_ago: now.duration_since(p.last_seen).as_secs(),
            })
            .collect()
    }

    /// Return all healthy pods (for distribution).
    pub async fn healthy_pods(&self) -> Vec<RegisteredPod> {
        let pods = self.pods.read().await;
        pods.values().filter(|p| p.healthy).cloned().collect()
    }

    /// Run the health check loop. Periodically pings each pod's
    /// `GET /api/v1/health` endpoint. Marks unhealthy pods and prunes
    /// those that have been unreachable for too long.
    pub async fn run_health_checks(&self, shutdown: CancellationToken) {
        info!("pod registry health checker started");

        loop {
            tokio::select! {
                _ = tokio::time::sleep(HEALTH_CHECK_INTERVAL) => {}
                _ = shutdown.cancelled() => {
                    debug!("pod registry health checker shutting down");
                    return;
                }
            }

            let pod_names: Vec<(String, String, Option<String>)> = {
                let pods = self.pods.read().await;
                pods.values()
                    .map(|p| (p.name.clone(), p.coop_url.clone(), p.auth_token.clone()))
                    .collect()
            };

            for (name, url, token) in &pod_names {
                let health_url = format!("{url}/api/v1/health");
                let mut req = self.http_client.get(&health_url);
                if let Some(ref t) = token {
                    req = req.header("Authorization", format!("Bearer {t}"));
                }

                let healthy = match req.send().await {
                    Ok(resp) => resp.status().is_success(),
                    Err(_) => false,
                };

                let mut pods = self.pods.write().await;
                if let Some(pod) = pods.get_mut(name) {
                    if healthy {
                        pod.healthy = true;
                        pod.last_seen = Instant::now();
                    } else {
                        pod.healthy = false;
                        let unreachable_for = pod.last_seen.elapsed();
                        if unreachable_for > PRUNE_AFTER {
                            warn!(
                                pod = name,
                                secs = unreachable_for.as_secs(),
                                "pruning unreachable pod"
                            );
                            pods.remove(name);
                        } else {
                            debug!(pod = name, "health check failed, marking unhealthy");
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(name: &str, url: &str) -> RegisterRequest {
        RegisterRequest {
            name: name.to_owned(),
            coop_url: url.to_owned(),
            profiles_needed: vec![],
            auth_token: None,
        }
    }

    #[tokio::test]
    async fn register_new_pod() {
        let registry = PodRegistry::new();
        assert!(registry.register(reg("mayor", "http://10.0.0.1:3000")).await);
        assert!(!registry.register(reg("mayor", "http://10.0.0.1:3000")).await); // re-register

        let pods = registry.list().await;
        assert_eq!(pods.len(), 1);
        assert_eq!(pods[0].name, "mayor");
        assert!(pods[0].healthy);
    }

    #[tokio::test]
    async fn deregister_removes_pod() {
        let registry = PodRegistry::new();
        registry.register(reg("mayor", "http://10.0.0.1:3000")).await;
        assert!(registry.deregister("mayor").await);
        assert!(!registry.deregister("mayor").await); // already gone
        assert!(registry.list().await.is_empty());
    }

    #[tokio::test]
    async fn healthy_pods_filters() {
        let registry = PodRegistry::new();
        registry.register(reg("good", "http://10.0.0.1:3000")).await;
        registry.register(reg("bad", "http://10.0.0.2:3000")).await;

        // Mark bad as unhealthy.
        {
            let mut pods = registry.pods.write().await;
            pods.get_mut("bad").map(|p| p.healthy = false);
        }

        let healthy = registry.healthy_pods().await;
        assert_eq!(healthy.len(), 1);
        assert_eq!(healthy[0].name, "good");
    }

    #[tokio::test]
    async fn profiles_needed_stored() {
        let registry = PodRegistry::new();
        registry
            .register(RegisterRequest {
                name: "worker".to_owned(),
                coop_url: "http://10.0.0.1:3000".to_owned(),
                profiles_needed: vec!["personal".to_owned(), "work".to_owned()],
                auth_token: Some("secret".to_owned()),
            })
            .await;

        let pods = registry.list().await;
        assert_eq!(pods[0].profiles_needed, vec!["personal", "work"]);
    }

    #[tokio::test]
    async fn re_register_updates_url() {
        let registry = PodRegistry::new();
        registry.register(reg("mayor", "http://old:3000")).await;
        registry.register(reg("mayor", "http://new:3000")).await;

        let pods = registry.list().await;
        assert_eq!(pods.len(), 1);
        assert_eq!(pods[0].coop_url, "http://new:3000");
    }
}
