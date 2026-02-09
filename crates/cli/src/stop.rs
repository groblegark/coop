// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Agent-agnostic stop hook configuration and gating logic.
//!
//! The stop hook becomes a gating HTTP call: the hook script `curl`s coop,
//! which returns a verdict (`{}` for allow, `{"decision":"block","reason":"..."}`
//! for block). A signal endpoint lets orchestrators unblock the next stop check.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, RwLock};

// ---------------------------------------------------------------------------
// Stop configuration
// ---------------------------------------------------------------------------

/// Top-level stop hook configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopConfig {
    /// How to handle stop hook calls.
    #[serde(default)]
    pub mode: StopMode,
    /// Custom prompt text included in the block reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Schema describing the expected signal body fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<StopSchema>,
}

impl Default for StopConfig {
    fn default() -> Self {
        Self {
            mode: StopMode::Allow,
            prompt: None,
            schema: None,
        }
    }
}

/// Stop hook mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopMode {
    /// Always allow the agent to stop (default behavior).
    #[default]
    Allow,
    /// Block stops until a signal is received via the signal endpoint.
    Signal,
}

/// Schema describing expected fields in the signal body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopSchema {
    /// Named fields the signal body should contain.
    pub fields: BTreeMap<String, StopSchemaField>,
}

/// A single field in the stop schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopSchemaField {
    /// Whether this field is required.
    #[serde(default)]
    pub required: bool,
    /// Allowed values (if restricted to an enum).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "enum")]
    pub r#enum: Option<Vec<String>>,
    /// Per-value descriptions for enum fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptions: Option<BTreeMap<String, String>>,
    /// Field-level description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// Stop events
// ---------------------------------------------------------------------------

/// A stop verdict event emitted to WebSocket/gRPC consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopEvent {
    /// What happened at the stop check.
    pub stop_type: StopType,
    /// Signal body (when stop_type is Signaled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<Value>,
    /// Error details (when stop_type is Error).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
    /// Monotonic sequence number.
    pub seq: u64,
}

/// Classification of a stop verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopType {
    /// Signal was received; agent is allowed to stop.
    Signaled,
    /// Agent is in an unrecoverable error state; allowed to stop.
    Error,
    /// Claude's safety valve (`stop_hook_active`) triggered; must allow.
    SafetyValve,
    /// Stop was blocked; agent should continue working.
    Blocked,
    /// Mode is `allow`; agent is always allowed to stop.
    Allowed,
}

impl StopType {
    /// Wire-format string for this type.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Signaled => "signaled",
            Self::Error => "error",
            Self::SafetyValve => "safety_valve",
            Self::Blocked => "blocked",
            Self::Allowed => "allowed",
        }
    }
}

impl std::fmt::Display for StopType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Stop state (runtime)
// ---------------------------------------------------------------------------

/// Runtime state for the stop hook gating system.
pub struct StopState {
    /// Mutable stop config (can be changed at runtime via API).
    pub config: RwLock<StopConfig>,
    /// Fast check: has a signal been received?
    pub signaled: AtomicBool,
    /// The signal body stored by the signal endpoint.
    pub signal_body: RwLock<Option<Value>>,
    /// Broadcast channel for stop events.
    pub stop_tx: broadcast::Sender<StopEvent>,
    /// Precomputed signal URL for block reason generation.
    pub signal_url: String,
    /// Monotonic sequence counter for stop events.
    pub stop_seq: std::sync::atomic::AtomicU64,
}

impl StopState {
    /// Create a new `StopState` with the given initial config and signal URL.
    pub fn new(config: StopConfig, signal_url: String) -> Self {
        let (stop_tx, _) = broadcast::channel(64);
        Self {
            config: RwLock::new(config),
            signaled: AtomicBool::new(false),
            signal_body: RwLock::new(None),
            stop_tx,
            signal_url,
            stop_seq: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Emit a stop event to all subscribers and return it.
    pub fn emit(
        &self,
        stop_type: StopType,
        signal: Option<Value>,
        error_detail: Option<String>,
    ) -> StopEvent {
        let seq = self
            .stop_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let event = StopEvent {
            stop_type,
            signal,
            error_detail,
            seq,
        };
        // Ignore send errors (no receivers is fine).
        let _ = self.stop_tx.send(event.clone());
        event
    }
}

impl std::fmt::Debug for StopState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StopState")
            .field(
                "signaled",
                &self.signaled.load(std::sync::atomic::Ordering::Relaxed),
            )
            .field("signal_url", &self.signal_url)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Block reason generation
// ---------------------------------------------------------------------------

/// Assemble the block reason text from the stop config and signal URL.
///
/// This is the `reason` field returned in `{"decision":"block","reason":"..."}`.
/// It tells the agent what to do: call the signal endpoint with the right body.
pub fn generate_block_reason(config: &StopConfig, signal_url: &str) -> String {
    let mut parts = Vec::new();

    // Custom prompt
    if let Some(ref prompt) = config.prompt {
        parts.push(prompt.clone());
    } else {
        parts.push("Do not stop yet. Signal when ready to stop.".to_owned());
    }

    // Schema fields documentation
    if let Some(ref schema) = config.schema {
        parts.push(String::new());
        parts.push("Signal body fields:".to_owned());
        for (name, field) in &schema.fields {
            let mut desc = format!("  - {name}");
            if field.required {
                desc.push_str(" (required)");
            }
            if let Some(ref d) = field.description {
                desc.push_str(&format!(": {d}"));
            }
            parts.push(desc);

            if let Some(ref values) = field.r#enum {
                let descs = field.descriptions.as_ref();
                for v in values {
                    let mut line = format!("      - \"{v}\"");
                    if let Some(vd) = descs.and_then(|d| d.get(v)) {
                        line.push_str(&format!(": {vd}"));
                    }
                    parts.push(line);
                }
            }
        }
    }

    // Curl instruction
    parts.push(String::new());
    parts.push(format!(
        "To signal, run: curl -sf -X POST -H 'Content-Type: application/json' -d '{{\"your\":\"json\"}}' {signal_url}"
    ));

    parts.join("\n")
}

#[cfg(test)]
#[path = "stop_tests.rs"]
mod tests;
