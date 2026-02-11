// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! File-backed append-only event log for state transitions and hook events.
//!
//! Events are appended as JSONL to files in the session directory. Catchup
//! reads from the file and filters by sequence number — no in-memory buffer.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::event::{RawHookEvent, TransitionEvent};

/// File-backed append-only event log.
///
/// State transitions and hook events are appended as JSONL lines. Catchup
/// reads the file and filters by seq — no in-memory buffer needed.
///
/// The log is never truncated — events accumulate across credential switches
/// so reconnecting clients can always catch up from their last known position.
pub struct EventLog {
    state_path: Option<PathBuf>,
    hook_path: Option<PathBuf>,
    hook_seq: AtomicU64,
}

/// A serialized state transition entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionEntry {
    pub prev: String,
    pub next: String,
    pub seq: u64,
    pub cause: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message: Option<String>,
    pub timestamp_ms: u64,
}

/// A serialized hook event entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEntry {
    pub hook_seq: u64,
    pub json: serde_json::Value,
    pub timestamp_ms: u64,
}

/// Catchup response combining both event types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatchupResponse {
    pub state_events: Vec<TransitionEntry>,
    pub hook_events: Vec<HookEntry>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl EventLog {
    /// Create a new event log. If `session_dir` is `None` (tests/attach mode),
    /// no files are written and catchup always returns empty.
    pub fn new(session_dir: Option<&std::path::Path>) -> Self {
        let (state_path, hook_path) = match session_dir {
            Some(dir) => {
                // Ensure dir exists (best-effort).
                let _ = std::fs::create_dir_all(dir);
                (Some(dir.join("state_events.jsonl")), Some(dir.join("hook_events.jsonl")))
            }
            None => (None, None),
        };
        Self { state_path, hook_path, hook_seq: AtomicU64::new(0) }
    }

    /// Append a state transition event to the log file.
    pub fn push_transition(&self, event: &TransitionEvent) {
        let Some(ref path) = self.state_path else {
            return;
        };
        let entry = TransitionEntry {
            prev: event.prev.as_str().to_owned(),
            next: event.next.as_str().to_owned(),
            seq: event.seq,
            cause: event.cause.clone(),
            last_message: event.last_message.clone(),
            timestamp_ms: now_ms(),
        };
        let Ok(mut line) = serde_json::to_string(&entry) else {
            return;
        };
        line.push('\n');
        let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
            return;
        };
        let _ = file.write_all(line.as_bytes());
    }

    /// Append a hook event to the log file.
    pub fn push_hook(&self, event: &RawHookEvent) {
        let Some(ref path) = self.hook_path else {
            return;
        };
        let hook_seq = self.hook_seq.fetch_add(1, Ordering::Relaxed);
        let entry = HookEntry { hook_seq, json: event.json.clone(), timestamp_ms: now_ms() };
        let Ok(mut line) = serde_json::to_string(&entry) else {
            return;
        };
        line.push('\n');
        let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
            return;
        };
        let _ = file.write_all(line.as_bytes());
    }

    /// Read state transition events with seq > `since_seq`.
    pub fn catchup_state(&self, since_seq: u64) -> Vec<TransitionEntry> {
        let Some(ref path) = self.state_path else {
            return vec![];
        };
        let Ok(contents) = std::fs::read_to_string(path) else {
            return vec![];
        };
        contents
            .lines()
            .filter_map(|line| serde_json::from_str::<TransitionEntry>(line).ok())
            .filter(|e| e.seq > since_seq)
            .collect()
    }

    /// Read hook events with hook_seq > `since_hook_seq`.
    pub fn catchup_hooks(&self, since_hook_seq: u64) -> Vec<HookEntry> {
        let Some(ref path) = self.hook_path else {
            return vec![];
        };
        let Ok(contents) = std::fs::read_to_string(path) else {
            return vec![];
        };
        contents
            .lines()
            .filter_map(|line| serde_json::from_str::<HookEntry>(line).ok())
            .filter(|e| e.hook_seq > since_hook_seq)
            .collect()
    }
}

#[cfg(test)]
#[path = "event_log_tests.rs"]
mod tests;
