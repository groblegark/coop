// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! NATS event publisher — broadcasts coop events to NATS subjects.

use std::path::Path;

use serde_json::Value;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::transport::ws::{
    profile_event_to_msg, start_event_to_msg, stop_event_to_msg, transition_to_msg,
    usage_event_to_msg, ServerMessage,
};
use crate::transport::Store;

/// Authentication options for connecting to NATS.
#[derive(Debug, Default)]
pub struct NatsAuth {
    pub token: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub creds_path: Option<Box<Path>>,
}

/// Publishes coop events to NATS subjects as JSON.
///
/// Each published message is a JSON object with the `ServerMessage` fields
/// plus injected identity fields:
/// - `session_id` — current agent session ID (tracks switches)
/// - `k8s` — optional Kubernetes pod metadata (when running in K8s)
pub struct NatsPublisher {
    client: async_nats::Client,
    prefix: String,
    /// Static K8s metadata detected at construction time.
    /// `Value::Null` when not running in Kubernetes.
    k8s_metadata: Value,
}

impl NatsPublisher {
    /// Connect to the NATS server at `url` with optional authentication.
    pub async fn connect(url: &str, prefix: &str, auth: NatsAuth) -> anyhow::Result<Self> {
        let opts = build_connect_options(auth).await?;
        let client = opts.connect(url).await?;
        let k8s_metadata = crate::mux_client::detect_metadata();
        Ok(Self { client, prefix: prefix.to_owned(), k8s_metadata })
    }

    /// Subscribe to all broadcast channels and publish events until shutdown.
    pub async fn run(self, store: &Store, shutdown: CancellationToken) {
        let mut state_rx = store.channels.state_tx.subscribe();
        let mut prompt_rx = store.channels.prompt_tx.subscribe();
        let mut hook_rx = store.channels.hook_tx.subscribe();
        let mut stop_rx = store.stop.stop_tx.subscribe();
        let mut start_rx = store.start.start_tx.subscribe();
        let mut usage_rx = store.usage.usage_tx.subscribe();
        let mut profile_rx = store.profile.profile_tx.subscribe();

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                event = state_rx.recv() => {
                    self.handle_with(store, event, &format!("{}.state", self.prefix), |e| {
                        transition_to_msg(&e)
                    }).await;
                }
                event = prompt_rx.recv() => {
                    self.handle_with(store, event, &format!("{}.prompt", self.prefix), |e| {
                        ServerMessage::PromptOutcome {
                            source: e.source,
                            r#type: e.r#type,
                            subtype: e.subtype,
                            option: e.option,
                        }
                    }).await;
                }
                event = hook_rx.recv() => {
                    self.handle_with(store, event, &format!("{}.hook", self.prefix), |e| {
                        ServerMessage::HookRaw { data: e.json }
                    }).await;
                }
                event = stop_rx.recv() => {
                    self.handle_with(store, event, &format!("{}.stop", self.prefix), |e| {
                        stop_event_to_msg(&e)
                    }).await;
                }
                event = start_rx.recv() => {
                    self.handle_with(store, event, &format!("{}.start", self.prefix), |e| {
                        start_event_to_msg(&e)
                    }).await;
                }
                event = usage_rx.recv() => {
                    self.handle_with(store, event, &format!("{}.usage", self.prefix), |e| {
                        usage_event_to_msg(&e)
                    }).await;
                }
                event = profile_rx.recv() => {
                    self.handle_with(store, event, &format!("{}.profile", self.prefix), |e| {
                        profile_event_to_msg(&e)
                    }).await;
                }
            }
        }
    }

    /// Convert a domain event to a [`ServerMessage`], inject identity fields, and publish.
    async fn handle_with<T, F>(
        &self,
        store: &Store,
        result: Result<T, broadcast::error::RecvError>,
        subject: &str,
        convert: F,
    ) where
        F: FnOnce(T) -> ServerMessage,
    {
        match result {
            Ok(event) => {
                let msg = convert(event);
                let mut obj = match serde_json::to_value(&msg) {
                    Ok(Value::Object(map)) => map,
                    Ok(_) => return, // ServerMessage always serializes to an object
                    Err(e) => {
                        tracing::warn!("nats: failed to serialize event for {subject}: {e}");
                        return;
                    }
                };

                // Inject session identity.
                let session_id = store.session_id.read().await.clone();
                obj.insert("session_id".to_owned(), Value::String(session_id));

                // Inject K8s metadata when available.
                if let Value::Object(ref meta) = self.k8s_metadata {
                    for (k, v) in meta {
                        obj.insert(k.clone(), v.clone());
                    }
                }

                let payload = match serde_json::to_vec(&obj) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("nats: failed to serialize event for {subject}: {e}");
                        return;
                    }
                };
                if let Err(e) = self.client.publish(subject.to_owned(), payload.into()).await {
                    tracing::warn!("nats: publish to {subject} failed: {e}");
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::debug!("nats: {subject} subscriber lagged by {n}");
            }
            Err(broadcast::error::RecvError::Closed) => {
                tracing::debug!("nats: {subject} channel closed");
            }
        }
    }
}

/// Build `ConnectOptions` from the auth configuration.
///
/// Priority (first match wins):
/// 1. Credentials file (JWT/NKey — standard NATS 2.0 auth)
/// 2. Token
/// 3. Username/password
/// 4. No auth
async fn build_connect_options(auth: NatsAuth) -> anyhow::Result<async_nats::ConnectOptions> {
    if let Some(ref path) = auth.creds_path {
        return Ok(async_nats::ConnectOptions::with_credentials_file(path).await?);
    }
    if let Some(token) = auth.token {
        return Ok(async_nats::ConnectOptions::with_token(token));
    }
    if let Some(user) = auth.user {
        let pass = auth.password.unwrap_or_default();
        return Ok(async_nats::ConnectOptions::with_user_and_password(user, pass));
    }
    Ok(async_nats::ConnectOptions::new())
}
