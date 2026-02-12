// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Terminal multiplexer — maintains WebSocket connections to all registered
//! agent pods and multiplexes their state, screen, and credential events
//! for dashboard clients over a single connection.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::broker::registry::PodRegistry;

/// Events emitted by the aggregator, tagged with the source pod name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MuxEvent {
    /// A pod's agent state changed.
    State {
        pod: String,
        prev: String,
        next: String,
        seq: u64,
    },
    /// A pod's screen was updated.
    Screen {
        pod: String,
        lines: Vec<String>,
        cols: u16,
        rows: u16,
    },
    /// A pod's credential status changed.
    Credential {
        pod: String,
        account: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// A pod came online (registered or reconnected).
    PodOnline {
        pod: String,
        coop_url: String,
    },
    /// A pod went offline (deregistered or connection lost).
    PodOffline {
        pod: String,
    },
}

/// Cached state for a single pod.
#[derive(Debug, Clone, Default)]
pub struct PodCache {
    pub agent_state: Option<String>,
    pub screen_lines: Option<Vec<String>>,
    pub screen_cols: u16,
    pub screen_rows: u16,
    pub credential_status: Option<String>,
}

/// Aggregator hub — connects to all registered pods and fans out events.
pub struct Multiplexer {
    registry: Arc<PodRegistry>,
    event_tx: broadcast::Sender<MuxEvent>,
    cache: Arc<RwLock<HashMap<String, PodCache>>>,
    streams: RwLock<HashMap<String, PodStreamHandle>>,
}

struct PodStreamHandle {
    cancel: CancellationToken,
    #[allow(dead_code)]
    handle: JoinHandle<()>,
}

/// Initial backoff for reconnection attempts.
const RECONNECT_INITIAL: Duration = Duration::from_secs(1);
/// Maximum backoff for reconnection attempts.
const RECONNECT_MAX: Duration = Duration::from_secs(30);
/// How often to check the registry for new/removed pods.
const REGISTRY_POLL_INTERVAL: Duration = Duration::from_secs(5);

impl Multiplexer {
    /// Create a new aggregator hub.
    pub fn new(registry: Arc<PodRegistry>) -> (Arc<Self>, broadcast::Receiver<MuxEvent>) {
        let (event_tx, event_rx) = broadcast::channel(256);
        let hub = Arc::new(Self {
            registry,
            event_tx,
            cache: Arc::new(RwLock::new(HashMap::new())),
            streams: RwLock::new(HashMap::new()),
        });
        (hub, event_rx)
    }

    /// Subscribe to aggregated events.
    pub fn subscribe(&self) -> broadcast::Receiver<MuxEvent> {
        self.event_tx.subscribe()
    }

    /// Return cached state for all pods.
    pub async fn cached_state(&self) -> HashMap<String, PodCache> {
        self.cache.read().await.clone()
    }

    /// Run the aggregator loop. Periodically checks the pod registry for changes
    /// and maintains a WebSocket stream per registered pod.
    pub async fn run(self: &Arc<Self>, shutdown: CancellationToken) {
        info!("aggregator hub started");

        loop {
            tokio::select! {
                _ = tokio::time::sleep(REGISTRY_POLL_INTERVAL) => {
                    self.reconcile().await;
                }
                _ = shutdown.cancelled() => {
                    info!("aggregator hub shutting down");
                    // Cancel all pod streams.
                    let mut streams = self.streams.write().await;
                    for (_, handle) in streams.drain() {
                        handle.cancel.cancel();
                    }
                    return;
                }
            }
        }
    }

    /// Reconcile active streams with the pod registry.
    async fn reconcile(&self) {
        let registered = self.registry.list().await;
        let registered_names: std::collections::HashSet<String> =
            registered.iter().map(|p| p.name.clone()).collect();

        // Remove streams for pods no longer in the registry.
        {
            let mut streams = self.streams.write().await;
            let to_remove: Vec<String> = streams
                .keys()
                .filter(|name| !registered_names.contains(*name))
                .cloned()
                .collect();
            for name in to_remove {
                if let Some(handle) = streams.remove(&name) {
                    handle.cancel.cancel();
                    let _ = self.event_tx.send(MuxEvent::PodOffline { pod: name.clone() });
                    self.cache.write().await.remove(&name);
                    debug!(pod = %name, "removed pod stream");
                }
            }
        }

        // Add streams for new pods.
        for pod in &registered {
            let streams = self.streams.read().await;
            if streams.contains_key(&pod.name) {
                continue;
            }
            drop(streams);

            let cancel = CancellationToken::new();
            let pod_name = pod.name.clone();
            let coop_url = pod.coop_url.clone();
            let event_tx = self.event_tx.clone();
            let cache = Arc::clone(&self.cache);
            let cancel_clone = cancel.clone();

            let _ = event_tx.send(MuxEvent::PodOnline {
                pod: pod_name.clone(),
                coop_url: coop_url.clone(),
            });

            let handle = tokio::spawn(async move {
                run_pod_stream(pod_name, coop_url, event_tx, cache, cancel_clone).await;
            });

            self.streams.write().await.insert(
                pod.name.clone(),
                PodStreamHandle { cancel, handle },
            );
            debug!(pod = %pod.name, "started pod stream");
        }
    }
}

/// Run a persistent connection to a single pod's coop WebSocket.
/// Reconnects with exponential backoff on failure.
async fn run_pod_stream(
    pod_name: String,
    coop_url: String,
    event_tx: broadcast::Sender<MuxEvent>,
    cache: Arc<RwLock<HashMap<String, PodCache>>>,
    cancel: CancellationToken,
) {
    let mut backoff = RECONNECT_INITIAL;

    loop {
        if cancel.is_cancelled() {
            return;
        }

        // Convert HTTP URL to WebSocket URL.
        let ws_url = coop_url
            .replace("http://", "ws://")
            .replace("https://", "wss://");
        let ws_url = format!("{ws_url}/ws?subscribe=screen,state,credentials");

        debug!(pod = %pod_name, url = %ws_url, "connecting to pod");

        match connect_and_stream(&pod_name, &ws_url, &event_tx, &cache, &cancel).await {
            Ok(()) => {
                // Clean disconnect (cancel was triggered).
                return;
            }
            Err(e) => {
                warn!(pod = %pod_name, error = %e, "pod stream disconnected");
                let _ = event_tx.send(MuxEvent::PodOffline {
                    pod: pod_name.clone(),
                });
            }
        }

        // Backoff before reconnect.
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = cancel.cancelled() => return,
        }
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

/// Connect to a pod's WebSocket and stream events until disconnect or cancel.
async fn connect_and_stream(
    pod_name: &str,
    ws_url: &str,
    event_tx: &broadcast::Sender<MuxEvent>,
    cache: &Arc<RwLock<HashMap<String, PodCache>>>,
    cancel: &CancellationToken,
) -> Result<(), String> {
    use futures_util::StreamExt;

    let (ws, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .map_err(|e| format!("WS connect failed: {e}"))?;

    let (_, mut read) = ws.split();

    info!(pod = %pod_name, "connected to pod WebSocket");

    // Reset backoff on successful connect.
    let _ = event_tx.send(MuxEvent::PodOnline {
        pod: pod_name.to_owned(),
        coop_url: ws_url.replace("/ws?subscribe=screen,state,credentials", "").to_owned(),
    });

    loop {
        tokio::select! {
            msg = read.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => return Err(format!("WS read error: {e}")),
                    None => return Err("WS stream ended".to_owned()),
                };

                if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                        handle_pod_message(pod_name, &json, event_tx, cache).await;
                    }
                }
            }
            _ = cancel.cancelled() => return Ok(()),
        }
    }
}

/// Parse an incoming WS message from a pod and emit the appropriate MuxEvent.
async fn handle_pod_message(
    pod_name: &str,
    msg: &serde_json::Value,
    event_tx: &broadcast::Sender<MuxEvent>,
    cache: &Arc<RwLock<HashMap<String, PodCache>>>,
) {
    let event_type = msg.get("event").and_then(|e| e.as_str()).unwrap_or("");

    match event_type {
        "transition" => {
            let prev = msg.get("prev").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            let next = msg.get("next").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            let seq = msg.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);

            cache.write().await
                .entry(pod_name.to_owned())
                .or_default()
                .agent_state = Some(next.clone());

            let _ = event_tx.send(MuxEvent::State {
                pod: pod_name.to_owned(),
                prev,
                next,
                seq,
            });
        }
        "screen" => {
            let lines: Vec<String> = msg.get("lines")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let cols = msg.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
            let rows = msg.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;

            {
                let mut c = cache.write().await;
                let entry = c.entry(pod_name.to_owned()).or_default();
                entry.screen_lines = Some(lines.clone());
                entry.screen_cols = cols;
                entry.screen_rows = rows;
            }

            let _ = event_tx.send(MuxEvent::Screen {
                pod: pod_name.to_owned(),
                lines,
                cols,
                rows,
            });
        }
        "credential:status" => {
            let account = msg.get("account").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            let status = msg.get("status").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            let error = msg.get("error").and_then(|v| v.as_str()).map(String::from);

            cache.write().await
                .entry(pod_name.to_owned())
                .or_default()
                .credential_status = Some(status.clone());

            let _ = event_tx.send(MuxEvent::Credential {
                pod: pod_name.to_owned(),
                account,
                status,
                error,
            });
        }
        _ => {
            // Ignore other events for now.
        }
    }
}
