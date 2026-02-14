// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Per-session API usage tracking.
//!
//! Extracts token counts from Claude session log JSONL `result` entries
//! and accumulates them into a cumulative snapshot. Exposed via HTTP, WS,
//! and gRPC.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, RwLock};

/// Per-entry extraction from a single Claude API response.
#[derive(Debug, Clone, Default)]
pub struct UsageDelta {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cost_usd: f64,
    pub duration_api_ms: u64,
}

/// Cumulative session usage counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub total_cost_usd: f64,
    pub request_count: u64,
    pub total_api_ms: u64,
}

impl SessionUsage {
    /// Add a delta to the cumulative counters.
    pub fn accumulate(&mut self, delta: &UsageDelta) {
        self.input_tokens += delta.input_tokens;
        self.output_tokens += delta.output_tokens;
        self.cache_read_tokens += delta.cache_read_input_tokens;
        self.cache_write_tokens += delta.cache_creation_input_tokens;
        self.total_cost_usd += delta.cost_usd;
        self.total_api_ms += delta.duration_api_ms;
        self.request_count += 1;
    }
}

/// Broadcast payload for usage updates.
#[derive(Debug, Clone, Serialize)]
pub struct UsageEvent {
    pub cumulative: SessionUsage,
    pub seq: u64,
}

/// Shared usage state, safe to access from multiple tasks.
pub struct UsageState {
    usage: RwLock<SessionUsage>,
    pub usage_tx: broadcast::Sender<UsageEvent>,
    seq: AtomicU64,
}

impl Default for UsageState {
    fn default() -> Self {
        Self::new()
    }
}

impl UsageState {
    /// Create with default (zero) usage and a broadcast channel.
    pub fn new() -> Self {
        let (usage_tx, _) = broadcast::channel(64);
        Self { usage: RwLock::new(SessionUsage::default()), usage_tx, seq: AtomicU64::new(0) }
    }

    /// Accumulate a delta, broadcast the updated snapshot.
    pub async fn accumulate(&self, delta: UsageDelta) {
        let snapshot = {
            let mut usage = self.usage.write().await;
            usage.accumulate(&delta);
            usage.clone()
        };
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.usage_tx.send(UsageEvent { cumulative: snapshot, seq });
    }

    /// Read-lock and clone the current snapshot.
    pub async fn snapshot(&self) -> SessionUsage {
        self.usage.read().await.clone()
    }
}

/// Extract a [`UsageDelta`] from a Claude session log JSONL entry.
///
/// Looks for usage data at `json["usage"]` (legacy/result entries) or
/// `json["message"]["usage"]` (assistant entries). Returns `None` if
/// the entry has no usage data.
pub fn extract_usage_delta(json: &Value) -> Option<UsageDelta> {
    let usage = json.get("usage").or_else(|| json.get("message").and_then(|m| m.get("usage")))?;

    let input = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let output = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);

    // Skip entries with zero tokens (e.g. system entries without real usage).
    if input == 0 && output == 0 {
        return None;
    }

    let cache_creation =
        usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let cache_read = usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);

    // Cost may live at top level or inside usage.
    let cost = json
        .get("costUSD")
        .or_else(|| usage.get("costUSD"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    let duration = json
        .get("durationMs")
        .or_else(|| usage.get("durationMs"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    Some(UsageDelta {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: cache_creation,
        cache_read_input_tokens: cache_read,
        cost_usd: cost,
        duration_api_ms: duration,
    })
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
