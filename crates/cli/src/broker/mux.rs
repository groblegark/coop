// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Terminal multiplexer — maintains WebSocket connections to all registered
//! agent pods and multiplexes their state, screen, and credential events
//! for dashboard clients over a single connection.
//!
//! Uses shared event types from `coop_mux::events` to stay in sync with the
//! standalone `coop-mux` binary.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::broker::registry::PodRegistry;

// Re-export shared types so existing `use crate::broker::mux::MuxEvent` works.
pub use coop_mux::events::{Aggregator, MuxEvent, SessionCache};
pub use coop_mux::events::{build_upstream_ws_url, parse_upstream_message};

/// Aggregator hub — connects to all registered pods and fans out events.
pub struct Multiplexer {
    registry: Arc<PodRegistry>,
    event_tx: broadcast::Sender<MuxEvent>,
    cache: Arc<RwLock<HashMap<String, SessionCache>>>,
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
    pub async fn cached_state(&self) -> HashMap<String, SessionCache> {
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
            let to_remove: Vec<String> =
                streams.keys().filter(|name| !registered_names.contains(*name)).cloned().collect();
            for name in to_remove {
                if let Some(handle) = streams.remove(&name) {
                    handle.cancel.cancel();
                    let _ = self.event_tx.send(MuxEvent::SessionOffline { session: name.clone() });
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

            let _ = event_tx
                .send(MuxEvent::SessionOnline { session: pod_name.clone(), url: coop_url.clone() });

            let handle = tokio::spawn(async move {
                run_pod_stream(pod_name, coop_url, event_tx, cache, cancel_clone).await;
            });

            self.streams.write().await.insert(pod.name.clone(), PodStreamHandle { cancel, handle });
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
    cache: Arc<RwLock<HashMap<String, SessionCache>>>,
    cancel: CancellationToken,
) {
    let mut backoff = RECONNECT_INITIAL;

    loop {
        if cancel.is_cancelled() {
            return;
        }

        let ws_url = build_upstream_ws_url(&coop_url, None);

        debug!(pod = %pod_name, url = %ws_url, "connecting to pod");

        match connect_and_stream(&pod_name, &ws_url, &event_tx, &cache, &cancel).await {
            Ok(()) => {
                // Clean disconnect (cancel was triggered).
                return;
            }
            Err(e) => {
                warn!(pod = %pod_name, error = %e, "pod stream disconnected");
                let _ = event_tx.send(MuxEvent::SessionOffline { session: pod_name.clone() });
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
    cache: &Arc<RwLock<HashMap<String, SessionCache>>>,
    cancel: &CancellationToken,
) -> Result<(), String> {
    use futures_util::StreamExt;

    let (ws, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .map_err(|e| format!("WS connect failed: {e}"))?;

    let (_, mut read) = ws.split();

    info!(pod = %pod_name, "connected to pod WebSocket");

    // Reset backoff on successful connect.
    let _ = event_tx.send(MuxEvent::SessionOnline {
        session: pod_name.to_owned(),
        url: ws_url.replace("/ws?subscribe=screen,state,credentials", "").to_owned(),
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
                        parse_upstream_message(pod_name, &json, event_tx, cache).await;
                    }
                }
            }
            _ = cancel.cancelled() => return Ok(()),
        }
    }
}
