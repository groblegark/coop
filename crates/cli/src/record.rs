// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Session recording — captures screen snapshots at semantic events.
//!
//! Records state transitions and hook events with full screen snapshots
//! as JSONL to `<session-dir>/recording.jsonl`. First line is a header,
//! subsequent lines are entries.

use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;

use crate::event::{RawHookEvent, TransitionEvent};
use crate::screen::ScreenSnapshot;
use crate::transport::state::TerminalState;

/// File-backed session recording state.
///
/// Captures screen snapshots at semantic events (state transitions, hook events)
/// and writes them as JSONL entries. The recording can be toggled at runtime.
pub struct RecordingState {
    enabled: AtomicBool,
    started_at: Mutex<Option<Instant>>,
    started_at_unix_ms: AtomicU64,
    path: Option<PathBuf>,
    seq: AtomicU64,
    header_written: AtomicBool,
    cols: AtomicU16,
    rows: AtomicU16,
    pub record_tx: broadcast::Sender<RecordingEntry>,
}

/// A single recording entry (broadcast + serialized to JSONL).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingEntry {
    pub ts: u64,
    pub seq: u64,
    pub kind: String,
    pub detail: serde_json::Value,
    pub screen: ScreenSnapshot,
}

/// Recording header written as the first JSONL line.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecordingHeader {
    version: u32,
    cols: u16,
    rows: u16,
    timestamp: u64,
}

/// Status snapshot returned by the status endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingStatus {
    pub enabled: bool,
    pub path: Option<String>,
    pub entries: u64,
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl RecordingState {
    /// Create a new recording state.
    ///
    /// If `session_dir` is `None` (tests/attach mode), no files are written.
    pub fn new(session_dir: Option<&std::path::Path>, cols: u16, rows: u16) -> Self {
        let path = session_dir.map(|dir| {
            let _ = std::fs::create_dir_all(dir);
            dir.join("recording.jsonl")
        });
        let (record_tx, _) = broadcast::channel(256);
        Self {
            enabled: AtomicBool::new(false),
            started_at: Mutex::new(None),
            started_at_unix_ms: AtomicU64::new(0),
            path,
            seq: AtomicU64::new(0),
            header_written: AtomicBool::new(false),
            cols: AtomicU16::new(cols),
            rows: AtomicU16::new(rows),
            record_tx,
        }
    }

    /// Enable recording. Writes the header on first enable.
    pub async fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
        let mut started = self.started_at.lock().await;
        if started.is_none() {
            *started = Some(Instant::now());
            self.started_at_unix_ms.store(now_unix_ms(), Ordering::Release);
        }
        self.write_header_once();
    }

    /// Disable recording.
    pub fn disable(&self) {
        self.enabled.store(false, Ordering::Release);
    }

    /// Whether recording is currently enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Append a recording entry with the given kind, detail, and screen snapshot.
    pub async fn push(&self, kind: &str, detail: serde_json::Value, screen: &ScreenSnapshot) {
        if !self.is_enabled() {
            return;
        }

        let ts = {
            let started = self.started_at.lock().await;
            match *started {
                Some(ref instant) => instant.elapsed().as_millis() as u64,
                None => 0,
            }
        };

        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;

        let entry =
            RecordingEntry { ts, seq, kind: kind.to_owned(), detail, screen: screen.clone() };

        // Write to file
        self.append_entry(&entry);

        // Broadcast to subscribers
        let _ = self.record_tx.send(entry);
    }

    /// Return the current recording status.
    pub fn status(&self) -> RecordingStatus {
        RecordingStatus {
            enabled: self.is_enabled(),
            path: self.path.as_ref().map(|p| p.display().to_string()),
            entries: self.seq.load(Ordering::Relaxed),
        }
    }

    /// Read entries from the recording file with seq > `since_seq`.
    pub fn catchup(&self, since_seq: u64) -> Vec<RecordingEntry> {
        let Some(ref path) = self.path else {
            return vec![];
        };
        let Ok(contents) = std::fs::read_to_string(path) else {
            return vec![];
        };
        contents
            .lines()
            .filter_map(|line| serde_json::from_str::<RecordingEntry>(line).ok())
            .filter(|e| e.seq > since_seq)
            .collect()
    }

    /// Read the full recording file contents.
    pub fn download(&self) -> Option<Vec<u8>> {
        let path = self.path.as_ref()?;
        std::fs::read(path).ok()
    }

    /// Write the header line to the recording file (once).
    fn write_header_once(&self) {
        if self.header_written.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(ref path) = self.path else {
            return;
        };
        let header = RecordingHeader {
            version: 1,
            cols: self.cols.load(Ordering::Relaxed),
            rows: self.rows.load(Ordering::Relaxed),
            timestamp: self.started_at_unix_ms.load(Ordering::Acquire),
        };
        let Ok(mut line) = serde_json::to_string(&header) else {
            return;
        };
        line.push('\n');
        let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
            return;
        };
        let _ = file.write_all(line.as_bytes());
    }

    /// Append a single entry to the recording file.
    fn append_entry(&self, entry: &RecordingEntry) {
        let Some(ref path) = self.path else {
            return;
        };
        let Ok(mut line) = serde_json::to_string(entry) else {
            return;
        };
        line.push('\n');
        let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
            return;
        };
        let _ = file.write_all(line.as_bytes());
    }
}

/// Hash a screen snapshot's visible content for deduplication.
fn screen_hash(snap: &ScreenSnapshot) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    snap.lines.hash(&mut hasher);
    snap.cursor.row.hash(&mut hasher);
    snap.cursor.col.hash(&mut hasher);
    snap.alt_screen.hash(&mut hasher);
    hasher.finish()
}

/// Spawn the recording subscriber task.
///
/// Listens to state transitions and hook events, capturing screen snapshots
/// at each. Also takes a periodic "screen" snapshot 30s after the last hook
/// event to capture intermediate activity while the agent is working.
///
/// Duplicate screen captures (identical content) are suppressed via hashing.
pub fn spawn_subscriber(
    record: Arc<RecordingState>,
    terminal: Arc<TerminalState>,
    state_tx: &broadcast::Sender<TransitionEvent>,
    hook_tx: &broadcast::Sender<RawHookEvent>,
    shutdown: CancellationToken,
) {
    let mut state_rx = state_tx.subscribe();
    let mut hook_rx = hook_tx.subscribe();
    let screen_delay = std::time::Duration::from_secs(30);
    tokio::spawn(async move {
        let screen_sleep = tokio::time::sleep(screen_delay);
        tokio::pin!(screen_sleep);
        let mut screen_armed = false;
        let mut last_hash: u64 = 0;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                event = state_rx.recv() => {
                    match event {
                        Ok(e) => {
                            if record.is_enabled() {
                                let snap = terminal.screen.read().await.snapshot();
                                let h = screen_hash(&snap);
                                if h != last_hash {
                                    last_hash = h;
                                    let detail = serde_json::json!({
                                        "prev": e.prev.as_str(),
                                        "next": e.next.as_str(),
                                        "cause": e.cause,
                                    });
                                    record.push("state", detail, &snap).await;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("recording: state subscriber lagged by {n}");
                        }
                        Err(_) => break,
                    }
                }
                event = hook_rx.recv() => {
                    match event {
                        Ok(e) => {
                            if record.is_enabled() {
                                let snap = terminal.screen.read().await.snapshot();
                                let h = screen_hash(&snap);
                                if h != last_hash {
                                    last_hash = h;
                                    let detail = serde_json::json!({
                                        "hook_seq": 0,
                                        "json": e.json,
                                    });
                                    record.push("hook", detail, &snap).await;
                                }
                                // Reset the periodic timer on each hook event.
                                screen_sleep.as_mut().reset(tokio::time::Instant::now() + screen_delay);
                                screen_armed = true;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("recording: hook subscriber lagged by {n}");
                        }
                        Err(_) => break,
                    }
                }
                _ = &mut screen_sleep, if screen_armed => {
                    screen_armed = false;
                    if record.is_enabled() {
                        let snap = terminal.screen.read().await.snapshot();
                        let h = screen_hash(&snap);
                        if h != last_hash {
                            last_hash = h;
                            record.push("screen", serde_json::json!({}), &snap).await;
                        }
                    }
                }
            }
        }
    });
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
