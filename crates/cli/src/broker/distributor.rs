// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential distribution to agent pods (Epic 16c).
//!
//! Listens for `CredentialEvent::Refreshed` from the broker and pushes
//! fresh credentials to all registered pods via their coop profile API.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::credential::{CredentialBroker, CredentialEvent};

use super::registry::{PodRegistry, RegisteredPod};

/// Maximum concurrent push operations.
const MAX_CONCURRENT: usize = 10;

/// Per-pod push timeout.
const PUSH_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum retries per pod per distribution round.
const MAX_RETRIES: u32 = 2;

/// Result of pushing credentials to a single pod.
#[derive(Debug, Clone, Serialize)]
pub struct PushResult {
    pub pod: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether a session switch was triggered (vs just profile registration).
    pub switched: bool,
}

/// Distributes credentials to agent pods.
pub struct Distributor {
    registry: Arc<PodRegistry>,
    broker: Arc<CredentialBroker>,
    http_client: reqwest::Client,
}

/// Profile entry for the coop session profiles API.
#[derive(Debug, Serialize)]
struct ProfilePayload {
    profiles: Vec<ProfileEntry>,
}

#[derive(Debug, Serialize)]
struct ProfileEntry {
    name: String,
    credentials: HashMap<String, String>,
}

/// Agent state response from `GET /api/v1/agent`.
#[derive(Debug, Deserialize)]
struct AgentResponse {
    #[serde(default)]
    state: String,
}

/// Switch request for `POST /api/v1/session/switch`.
#[derive(Debug, Serialize)]
struct SwitchPayload {
    profile: String,
    force: bool,
}

impl Distributor {
    pub fn new(registry: Arc<PodRegistry>, broker: Arc<CredentialBroker>) -> Self {
        Self {
            registry,
            broker,
            http_client: reqwest::Client::builder()
                .timeout(PUSH_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// Run the distribution loop — listens for credential events and pushes
    /// to all registered pods.
    pub async fn run(
        &self,
        mut credential_rx: broadcast::Receiver<CredentialEvent>,
        shutdown: CancellationToken,
    ) {
        info!("credential distributor started");

        loop {
            let event = tokio::select! {
                event = credential_rx.recv() => {
                    match event {
                        Ok(e) => e,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("credential distributor lagged {n} events");
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            debug!("credential event channel closed");
                            break;
                        }
                    }
                }
                _ = shutdown.cancelled() => {
                    debug!("credential distributor shutting down");
                    break;
                }
            };

            if let CredentialEvent::Refreshed { ref account, .. } = event {
                info!(account = account.as_str(), "distributing refreshed credentials to pods");
                let results = self.distribute(account).await;
                let success = results.iter().filter(|r| r.success).count();
                let failed = results.iter().filter(|r| !r.success).count();
                info!(
                    account = account.as_str(),
                    success,
                    failed,
                    total = results.len(),
                    "distribution complete"
                );
            }
        }
    }

    /// Push all healthy account credentials to all healthy pods.
    /// Returns a result per pod.
    pub async fn distribute(&self, _refreshed_account: &str) -> Vec<PushResult> {
        let pods = self.registry.healthy_pods().await;
        if pods.is_empty() {
            debug!("no healthy pods to distribute to");
            return vec![];
        }

        // Build the full profile set from all healthy accounts.
        let all_creds = self.broker.all_credentials().await;
        if all_creds.is_empty() {
            debug!("no credentials available to distribute");
            return vec![];
        }

        // Use a semaphore to limit concurrency.
        let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT));
        let mut handles = Vec::with_capacity(pods.len());

        for pod in pods {
            let sem = Arc::clone(&semaphore);
            let client = self.http_client.clone();
            let creds = filter_for_pod(&pod, &all_creds);
            let refreshed = _refreshed_account.to_owned();

            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await;
                push_to_pod(&client, &pod, &creds, &refreshed).await
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => {
                    error!("distribution task panicked: {e}");
                }
            }
        }

        results
    }
}

/// Filter credentials to only those the pod needs.
fn filter_for_pod(
    pod: &RegisteredPod,
    all_creds: &[(String, HashMap<String, String>)],
) -> Vec<(String, HashMap<String, String>)> {
    if pod.profiles_needed.is_empty() {
        // Pod wants all profiles.
        return all_creds.to_vec();
    }
    all_creds.iter().filter(|(name, _)| pod.profiles_needed.contains(name)).cloned().collect()
}

/// Push credentials to a single pod. Registers profiles, then optionally
/// triggers a switch if the agent is idle.
async fn push_to_pod(
    client: &reqwest::Client,
    pod: &RegisteredPod,
    creds: &[(String, HashMap<String, String>)],
    refreshed_account: &str,
) -> PushResult {
    let pod_name = pod.name.clone();

    if creds.is_empty() {
        return PushResult { pod: pod_name, success: true, error: None, switched: false };
    }

    // 1. Register profiles.
    let profiles: Vec<ProfileEntry> = creds
        .iter()
        .map(|(name, c)| ProfileEntry { name: name.clone(), credentials: c.clone() })
        .collect();

    let payload = ProfilePayload { profiles };
    let profiles_url = format!("{}/api/v1/session/profiles", pod.coop_url);

    let mut req = client.post(&profiles_url);
    if let Some(ref token) = pod.auth_token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }

    for attempt in 0..=MAX_RETRIES {
        let result = req.try_clone().map(|r| r.json(&payload).send());

        match result {
            Some(fut) => match fut.await {
                Ok(resp) if resp.status().is_success() => {
                    debug!(pod = pod_name.as_str(), "profiles registered");
                    break;
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    if attempt == MAX_RETRIES {
                        return PushResult {
                            pod: pod_name,
                            success: false,
                            error: Some(format!(
                                "profile registration failed: HTTP {status}: {body}"
                            )),
                            switched: false,
                        };
                    }
                    warn!(
                        pod = pod_name.as_str(),
                        attempt, "profile registration failed: {status}"
                    );
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(e) => {
                    if attempt == MAX_RETRIES {
                        return PushResult {
                            pod: pod_name,
                            success: false,
                            error: Some(format!("profile registration failed: {e}")),
                            switched: false,
                        };
                    }
                    warn!(pod = pod_name.as_str(), attempt, "profile push error: {e}");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            },
            None => {
                return PushResult {
                    pod: pod_name,
                    success: false,
                    error: Some("failed to clone request".into()),
                    switched: false,
                };
            }
        }
    }

    // 2. Check if agent is idle — if so, switch to the refreshed profile.
    let agent_url = format!("{}/api/v1/agent", pod.coop_url);
    let mut agent_req = client.get(&agent_url);
    if let Some(ref token) = pod.auth_token {
        agent_req = agent_req.header("Authorization", format!("Bearer {token}"));
    }

    let should_switch = match agent_req.send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<AgentResponse>().await {
            Ok(agent) => {
                matches!(agent.state.as_str(), "idle" | "waiting_for_input" | "exited")
            }
            Err(_) => false,
        },
        _ => false,
    };

    if should_switch {
        let switch_url = format!("{}/api/v1/session/switch", pod.coop_url);
        let switch_payload = SwitchPayload { profile: refreshed_account.to_owned(), force: false };

        let mut switch_req = client.post(&switch_url);
        if let Some(ref token) = pod.auth_token {
            switch_req = switch_req.header("Authorization", format!("Bearer {token}"));
        }

        match switch_req.json(&switch_payload).send().await {
            Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 202 => {
                info!(pod = pod_name.as_str(), "session switched to refreshed profile");
                return PushResult { pod: pod_name, success: true, error: None, switched: true };
            }
            Ok(resp) => {
                debug!(
                    pod = pod_name.as_str(),
                    status = resp.status().as_u16(),
                    "switch not applied (profiles still registered)"
                );
            }
            Err(e) => {
                debug!(
                    pod = pod_name.as_str(),
                    "switch request failed: {e} (profiles still registered)"
                );
            }
        }
    }

    PushResult { pod: pod_name, success: true, error: None, switched: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_for_pod_all_profiles() {
        let pod = RegisteredPod {
            name: "mayor".into(),
            coop_url: "http://test".into(),
            profiles_needed: vec![],
            last_seen: std::time::Instant::now(),
            healthy: true,
            auth_token: None,
        };

        let creds = vec![
            ("personal".into(), HashMap::from([("KEY".into(), "val1".into())])),
            ("work".into(), HashMap::from([("KEY".into(), "val2".into())])),
        ];

        let filtered = filter_for_pod(&pod, &creds);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn filter_for_pod_specific_profiles() {
        let pod = RegisteredPod {
            name: "worker".into(),
            coop_url: "http://test".into(),
            profiles_needed: vec!["personal".into()],
            last_seen: std::time::Instant::now(),
            healthy: true,
            auth_token: None,
        };

        let creds = vec![
            ("personal".into(), HashMap::from([("KEY".into(), "val1".into())])),
            ("work".into(), HashMap::from([("KEY".into(), "val2".into())])),
        ];

        let filtered = filter_for_pod(&pod, &creds);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].0, "personal");
    }
}
