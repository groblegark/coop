// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! NATS JetStream receiver for hook events published by bd daemon.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use tracing::{debug, info, warn};

use super::HookEvent;

/// Configuration for connecting to a bd daemon NATS JetStream instance.
#[derive(Debug, Clone)]
pub struct NatsConfig {
    /// NATS server URL (e.g. "nats://127.0.0.1:4222").
    pub url: String,
    /// Auth token for NATS connection (BD_DAEMON_TOKEN).
    pub token: Option<String>,
    /// JetStream stream name (default: "HOOK_EVENTS").
    pub stream: String,
    /// Subject filter (default: "hooks.>").
    pub subject: String,
    /// Durable consumer name (unique per coop instance).
    pub consumer: String,
}

impl NatsConfig {
    /// Build config from environment variables and optional nats-info.json.
    ///
    /// Discovery order:
    /// 1. `COOP_NATS_URL` env var (explicit)
    /// 2. nats-info.json in `$BEADS_DIR/.runtime/` (auto-discovery)
    ///
    /// Token from `COOP_NATS_TOKEN` or `BD_DAEMON_TOKEN` or nats-info.json.
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("COOP_NATS_URL")
            .ok()
            .or_else(discover_nats_url)
            .filter(|u| !u.is_empty())?;

        let token = std::env::var("COOP_NATS_TOKEN")
            .ok()
            .or_else(|| std::env::var("BD_DAEMON_TOKEN").ok())
            .or_else(discover_nats_token)
            .filter(|t| !t.is_empty());

        let stream =
            std::env::var("COOP_NATS_STREAM").unwrap_or_else(|_| "HOOK_EVENTS".to_string());

        let subject = std::env::var("COOP_NATS_SUBJECT").unwrap_or_else(|_| "hooks.>".to_string());

        let consumer = std::env::var("COOP_NATS_CONSUMER")
            .unwrap_or_else(|_| format!("coop-{}", uuid::Uuid::new_v4()));

        Some(Self { url, token, stream, subject, consumer })
    }
}

/// NATS info file written by bd daemon at startup.
#[derive(Deserialize)]
struct NatsInfo {
    url: Option<String>,
    token: Option<String>,
    #[allow(dead_code)]
    port: Option<u16>,
}

/// Discover NATS URL from nats-info.json.
fn discover_nats_url() -> Option<String> {
    let path = nats_info_path()?;
    let info: NatsInfo = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    info.url
}

/// Discover NATS token from nats-info.json.
fn discover_nats_token() -> Option<String> {
    let path = nats_info_path()?;
    let info: NatsInfo = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    info.token
}

/// Find nats-info.json path from BEADS_DIR env.
fn nats_info_path() -> Option<PathBuf> {
    let beads_dir = std::env::var("BEADS_DIR").ok()?;
    let path = Path::new(&beads_dir).join(".runtime").join("nats-info.json");
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Receives hook events from a NATS JetStream subscription.
///
/// Parallels [`HookReceiver`](super::hook_recv::HookReceiver) but reads from
/// a NATS JetStream durable consumer instead of a named pipe. Events are
/// published by `bd daemon` to the `HOOK_EVENTS` stream on `hooks.*` subjects.
pub struct NatsReceiver {
    config: NatsConfig,
    messages: Option<async_nats::jetstream::consumer::pull::Stream>,
}

/// Intermediate type for parsing bd daemon NATS event payloads.
///
/// bd daemon publishes events with `hook_event_name` as the event type
/// discriminator, plus optional fields like `tool_name`, `tool_input`, etc.
#[derive(Deserialize)]
struct NatsHookPayload {
    hook_event_name: String,
    tool_name: Option<String>,
    tool_input: Option<serde_json::Value>,
    #[serde(default)]
    notification_type: Option<String>,
}

impl NatsReceiver {
    pub fn new(config: NatsConfig) -> Self {
        Self { config, messages: None }
    }

    /// Connect to NATS and create a JetStream pull consumer.
    pub async fn connect(&mut self) -> anyhow::Result<()> {
        let mut opts = async_nats::ConnectOptions::new();
        if let Some(ref token) = self.config.token {
            opts = opts.token(token.clone());
        }
        opts = opts.retry_on_initial_connect();

        info!(url = %self.config.url, stream = %self.config.stream, "connecting to NATS");
        let client = opts.connect(&self.config.url).await?;
        let jetstream = async_nats::jetstream::new(client);

        let stream = jetstream.get_stream(&self.config.stream).await?;
        debug!(stream = %self.config.stream, "NATS stream found");

        let consumer = stream
            .get_or_create_consumer(
                &self.config.consumer,
                async_nats::jetstream::consumer::pull::Config {
                    durable_name: Some(self.config.consumer.clone()),
                    filter_subject: self.config.subject.clone(),
                    deliver_policy: async_nats::jetstream::consumer::DeliverPolicy::New,
                    ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
                    ..Default::default()
                },
            )
            .await?;

        let messages = consumer
            .stream()
            .max_messages_per_batch(10)
            .heartbeat(Duration::from_secs(15))
            .messages()
            .await?;

        info!(consumer = %self.config.consumer, "NATS consumer ready");
        self.messages = Some(messages);
        Ok(())
    }

    /// Read the next hook event from the NATS subscription.
    ///
    /// Returns `None` if the subscription closes or an unrecoverable error
    /// occurs. Skips messages that don't parse as valid hook events.
    /// The second element is the raw JSON value for broadcasting.
    pub async fn next_event(&mut self) -> Option<(HookEvent, serde_json::Value)> {
        use futures_util::StreamExt;

        let messages = self.messages.as_mut()?;
        loop {
            match messages.next().await {
                Some(Ok(msg)) => {
                    // Acknowledge the message immediately so we don't
                    // re-process it on restart.
                    if let Err(e) = msg.ack().await {
                        warn!("NATS ack failed: {e}");
                    }
                    if let Some(pair) = parse_nats_payload(&msg.payload) {
                        return Some(pair);
                    }
                    // Unrecognized event type — skip and read next.
                }
                Some(Err(e)) => {
                    warn!("NATS message error: {e}");
                    // Transient errors — keep reading.
                }
                None => return None, // Stream closed.
            }
        }
    }
}

/// Parse a NATS message payload into a [`HookEvent`] and the raw JSON value.
///
/// bd daemon publishes events as JSON with `hook_event_name` as the
/// discriminator. Maps to the upstream `HookEvent` variants.
pub fn parse_nats_payload(payload: &[u8]) -> Option<(HookEvent, serde_json::Value)> {
    let raw_json: serde_json::Value = serde_json::from_slice(payload).ok()?;
    let raw: NatsHookPayload = serde_json::from_slice(payload).ok()?;
    let event = match raw.hook_event_name.as_str() {
        "PostToolUse" => {
            let tool = raw.tool_name.unwrap_or_default();
            HookEvent::ToolAfter { tool }
        }
        "Stop" => HookEvent::TurnEnd,
        "SessionEnd" => HookEvent::SessionEnd,
        "SessionStart" => HookEvent::SessionStart,
        "UserPromptSubmit" => HookEvent::TurnStart,
        "Notification" => {
            let notification_type = raw.notification_type?;
            HookEvent::Notification { notification_type }
        }
        "PreToolUse" => {
            let tool = raw.tool_name?;
            HookEvent::ToolBefore { tool, tool_input: raw.tool_input }
        }
        // Events we receive but don't map: PreCompact, SubagentStart, SubagentStop,
        // PostToolUseFailure — these don't change agent state.
        _ => return None,
    };
    Some((event, raw_json))
}

#[cfg(test)]
#[path = "nats_recv_tests.rs"]
mod tests;
