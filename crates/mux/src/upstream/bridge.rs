// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Bidirectional WebSocket bridge: single upstream connection multiplexed to N downstream clients.
//!
//! Features:
//! - **Per-client channels**: each downstream client gets a dedicated mpsc sender/receiver.
//! - **Correlation routing**: outgoing requests are stamped with a `request_id`; responses are
//!   routed back to the originating client only.
//! - **Subscription filtering**: streaming events are forwarded only to clients whose flags match.
//! - **Upstream write**: downstream messages are forwarded through the single upstream connection.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::state::SessionEntry;

/// Identifies a downstream client within the bridge.
pub type ClientId = u64;

/// Subscription flags controlling which upstream events reach a downstream client.
#[derive(Debug, Clone, Copy, Default)]
pub struct SubscriptionFlags {
    pub pty: bool,
    pub screen: bool,
    pub state: bool,
}

impl SubscriptionFlags {
    /// Parse a comma-separated flags string (e.g. `"pty,state"`).
    pub fn parse(s: &str) -> Self {
        let mut flags = Self::default();
        for token in s.split(',') {
            match token.trim() {
                "pty" | "output" => flags.pty = true,
                "screen" => flags.screen = true,
                "state" => flags.state = true,
                _ => {}
            }
        }
        flags
    }

    /// Return true if an upstream event with the given `"event"` tag should be forwarded.
    fn matches_event(&self, event: Option<&str>) -> bool {
        match event {
            Some("pty" | "output") => self.pty,
            Some("replay") => self.pty,
            Some("screen") => self.screen,
            Some("transition" | "exit" | "prompt:outcome" | "stop:outcome" | "start:outcome") => {
                self.state
            }
            // Responses (have request_id) and unknown events always pass through.
            _ => true,
        }
    }
}

/// Per-client bookkeeping within the bridge.
struct ClientSlot {
    tx: mpsc::UnboundedSender<Arc<str>>,
    flags: SubscriptionFlags,
}

/// In-flight request awaiting an upstream response.
struct PendingRequest {
    client_id: ClientId,
    /// Whether the downstream client included a `request_id` in its message.
    /// When false, we strip the bridge-assigned `request_id` from the response
    /// so the client sees it as a streaming event.
    client_had_rid: bool,
    /// Original downstream message text, retained so the request can be
    /// re-sent if the upstream connection drops before the response arrives.
    text: String,
}

/// Bidirectional WebSocket bridge for a single upstream session.
///
/// One upstream WS connection is shared across all downstream clients.  The bridge handles:
/// - Fan-out of streaming events filtered by per-client [`SubscriptionFlags`].
/// - Stamping outgoing requests with correlation IDs and routing responses to originators.
pub struct WsBridge {
    /// Send downstream-originated messages upstream (client_id, raw JSON text).
    upstream_tx: mpsc::UnboundedSender<(ClientId, String)>,
    /// Per-client state. Guarded for add/remove from outside the run loop.
    clients: Arc<RwLock<HashMap<ClientId, ClientSlot>>>,
    next_id: AtomicU64,
    cancel: CancellationToken,
}

impl WsBridge {
    /// Create and start a new bidirectional WS bridge for the given session.
    ///
    /// The bridge connects to upstream with `pty,state` subscriptions and filters
    /// per-client on the downstream side.
    pub fn connect(entry: &Arc<SessionEntry>) -> Arc<Self> {
        let cancel = entry.cancel.child_token();
        let clients: Arc<RwLock<HashMap<ClientId, ClientSlot>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let (upstream_tx, upstream_rx) = mpsc::unbounded_channel();

        let bridge = Arc::new(Self {
            upstream_tx,
            clients: Arc::clone(&clients),
            next_id: AtomicU64::new(1),
            cancel: cancel.clone(),
        });

        // The upstream subscription must cover the union of all possible client flags.
        let subscribe = "pty,state";
        let url = build_ws_url(&entry.url, entry.auth_token.as_deref(), subscribe);
        let entry_id = entry.id.clone();

        tokio::spawn(run_loop(url, entry_id, cancel, clients, upstream_rx));

        bridge
    }

    /// Register a new downstream client with the given subscription flags.
    ///
    /// Returns a `(ClientId, Receiver)` pair.  Messages matching the flags (and
    /// correlation-routed responses) arrive on the receiver.
    pub async fn add_client(
        &self,
        flags: SubscriptionFlags,
    ) -> (ClientId, mpsc::UnboundedReceiver<Arc<str>>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        self.clients.write().await.insert(id, ClientSlot { tx, flags });
        (id, rx)
    }

    /// Remove a downstream client.
    pub async fn remove_client(&self, id: ClientId) {
        self.clients.write().await.remove(&id);
    }

    /// Return the number of connected downstream clients.
    pub async fn client_count(&self) -> usize {
        self.clients.read().await.len()
    }

    /// Send a message from a downstream client through the upstream WS connection.
    ///
    /// The bridge stamps a `request_id` so the response can be correlation-routed
    /// back to this client.
    pub fn send_upstream(&self, client_id: ClientId, text: String) {
        let _ = self.upstream_tx.send((client_id, text));
    }
}

impl Drop for WsBridge {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn run_loop(
    url: String,
    entry_id: String,
    cancel: CancellationToken,
    clients: Arc<RwLock<HashMap<ClientId, ClientSlot>>>,
    mut downstream_rx: mpsc::UnboundedReceiver<(ClientId, String)>,
) {
    let mut backoff_ms = 100u64;
    let max_backoff_ms = 5000u64;
    let mut rid_counter: u64 = 0;
    // Pending correlation IDs: request_id -> PendingRequest.
    // Stores the original message text so orphaned requests can be re-sent on upstream reconnect.
    let mut pending: HashMap<String, PendingRequest> = HashMap::new();

    loop {
        if cancel.is_cancelled() {
            break;
        }

        match tokio_tungstenite::connect_async(&url).await {
            Ok((ws_stream, _)) => {
                backoff_ms = 100;
                tracing::debug!(session_id = %entry_id, "upstream WS connected");

                let (mut write, mut read) = ws_stream.split();

                // Re-send orphaned pending requests from the previous connection.
                // Their responses were lost when the old upstream dropped.
                let stale: Vec<PendingRequest> = pending.drain().map(|(_, req)| req).collect();
                for req in stale {
                    let stamped = stamp_request_id(&mut rid_counter, &req.text);
                    pending.insert(
                        stamped.request_id.clone(),
                        PendingRequest {
                            client_id: req.client_id,
                            client_had_rid: req.client_had_rid,
                            text: req.text,
                        },
                    );
                    if write.send(Message::Text(stamped.text.into())).await.is_err() {
                        tracing::debug!(session_id = %entry_id, "upstream WS write failed during resend");
                        break;
                    }
                }

                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => return,

                        // Upstream -> route to clients
                        msg = read.next() => {
                            match msg {
                                Some(Ok(Message::Text(text))) => {
                                    let text = text.to_string();
                                    let info = extract_route_info(&text);
                                    let shared: Arc<str> = Arc::from(text.as_str());

                                    if let Some(rid) = info.request_id {
                                        // Response: route to originator only.
                                        if let Some(req) = pending.remove(rid) {
                                            let guard = clients.read().await;
                                            if let Some(slot) = guard.get(&req.client_id) {
                                                // If the client sent a fire-and-forget (no request_id),
                                                // strip the bridge-assigned request_id so the client
                                                // sees it as a streaming event.
                                                let msg = if req.client_had_rid {
                                                    shared
                                                } else {
                                                    Arc::from(strip_request_id(&text))
                                                };
                                                let _ = slot.tx.send(msg);
                                            }
                                        }
                                    } else {
                                        // Streaming event: fan out by subscription flags.
                                        let guard = clients.read().await;
                                        for slot in guard.values() {
                                            if slot.flags.matches_event(info.event) {
                                                let _ = slot.tx.send(Arc::clone(&shared));
                                            }
                                        }
                                    }
                                }
                                Some(Ok(Message::Close(_))) | None => {
                                    tracing::debug!(session_id = %entry_id, "upstream WS closed");
                                    break;
                                }
                                Some(Err(e)) => {
                                    tracing::debug!(session_id = %entry_id, err = %e, "upstream WS error");
                                    break;
                                }
                                _ => {} // ping/pong/binary ignored
                            }
                        }

                        // Downstream -> stamp request_id, forward upstream
                        msg = downstream_rx.recv() => {
                            match msg {
                                Some((client_id, text)) => {
                                    let client_had_rid = extract_route_info(&text).request_id.is_some();
                                    let stamped = stamp_request_id(&mut rid_counter, &text);
                                    pending.insert(stamped.request_id.clone(), PendingRequest {
                                        client_id,
                                        client_had_rid,
                                        text,
                                    });
                                    if write.send(Message::Text(stamped.text.into())).await.is_err() {
                                        tracing::debug!(session_id = %entry_id, "upstream WS write failed");
                                        break;
                                    }
                                }
                                None => return, // bridge dropped
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::debug!(
                    session_id = %entry_id,
                    err = %e,
                    backoff_ms,
                    "upstream WS connect failed, retrying"
                );
            }
        }

        // Exponential backoff before reconnect.
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)) => {}
        }
        backoff_ms = (backoff_ms * 2).min(max_backoff_ms);
    }
}

/// Lightweight routing info extracted from a JSON message without full deserialization.
#[derive(Deserialize, Default)]
struct RouteInfo<'a> {
    #[serde(default)]
    event: Option<&'a str>,
    #[serde(default)]
    request_id: Option<&'a str>,
}

/// Extract the `event` and `request_id` fields from a JSON object.
fn extract_route_info(json: &str) -> RouteInfo<'_> {
    serde_json::from_str(json).unwrap_or_default()
}

/// Result of stamping a `request_id` onto an outgoing JSON message.
struct StampedMessage {
    text: String,
    request_id: String,
}

/// Replace (or insert) `request_id` in a JSON message with a bridge-assigned correlation ID.
///
/// Uses `serde_json::Value` to properly handle messages that already contain a `request_id`
/// from the downstream client, avoiding duplicate keys.
fn stamp_request_id(counter: &mut u64, json: &str) -> StampedMessage {
    *counter += 1;
    let rid = counter.to_string();

    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(json) {
        if let Some(obj) = value.as_object_mut() {
            obj.insert("request_id".to_owned(), serde_json::Value::String(rid.clone()));
        }
        let text = serde_json::to_string(&value).unwrap_or_else(|_| json.to_owned());
        return StampedMessage { text, request_id: rid };
    }

    // Fallback: string splice for non-JSON (shouldn't happen with well-formed messages).
    let text = if let Some(rest) = json.strip_prefix('{') {
        format!("{{\"request_id\":\"{rid}\",{rest}")
    } else {
        format!("{{\"request_id\":\"{rid}\",\"_raw\":{json}}}")
    };
    StampedMessage { text, request_id: rid }
}

/// Remove the `request_id` field from a JSON message.
///
/// Used when forwarding a response to a client that sent the original message without
/// a `request_id` (fire-and-forget). Stripping the bridge-assigned ID ensures the client
/// sees the response as a streaming event rather than an RPC response.
fn strip_request_id(json: &str) -> String {
    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(json) {
        if let Some(obj) = value.as_object_mut() {
            obj.remove("request_id");
        }
        return serde_json::to_string(&value).unwrap_or_else(|_| json.to_owned());
    }
    json.to_owned()
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;

/// Build the upstream WebSocket URL from an HTTP base URL.
fn build_ws_url(base_url: &str, auth_token: Option<&str>, subscribe: &str) -> String {
    let ws_base = if base_url.starts_with("https://") {
        base_url.replacen("https://", "wss://", 1)
    } else {
        base_url.replacen("http://", "ws://", 1)
    };

    let mut url = format!("{ws_base}/ws?subscribe={subscribe}");
    if let Some(token) = auth_token {
        url.push_str(&format!("&token={token}"));
    }
    url
}
