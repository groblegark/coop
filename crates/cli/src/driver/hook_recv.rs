// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};

use nix::sys::stat::Mode;
use serde::Deserialize;
use tokio::io::unix::AsyncFd;

use super::HookEvent;

/// Newtype for a FIFO file descriptor, for use with [`AsyncFd`].
struct FifoFd(OwnedFd);

impl AsRawFd for FifoFd {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0.as_raw_fd()
    }
}

/// Receives structured hook events from a named pipe (FIFO).
///
/// Agent hooks write JSON lines to the pipe. The receiver reads and parses
/// them into [`HookEvent`] values.
///
/// Uses non-blocking I/O via [`AsyncFd`] so that reads are cancellable
/// by `tokio::select!` and don't leak blocked threads on shutdown.
pub struct HookReceiver {
    pipe_path: PathBuf,
    async_fd: Option<AsyncFd<FifoFd>>,
    line_buf: Vec<u8>,
}

/// Intermediate type for parsing hook JSON from the pipe.
#[derive(Deserialize)]
struct RawHookEvent {
    event: String,
    data: Option<serde_json::Value>,
}

impl HookReceiver {
    /// Create a new hook receiver, creating the named pipe at `pipe_path`.
    pub fn new(pipe_path: &Path) -> anyhow::Result<Self> {
        nix::unistd::mkfifo(pipe_path, Mode::from_bits_truncate(0o600))?;
        Ok(Self {
            pipe_path: pipe_path.to_path_buf(),
            async_fd: None,
            line_buf: Vec::with_capacity(4096),
        })
    }

    /// Path to the named pipe.
    pub fn pipe_path(&self) -> &Path {
        &self.pipe_path
    }

    /// Read the next hook event from the pipe.
    ///
    /// Returns `None` on EOF or unrecoverable error. Skips malformed lines.
    pub async fn next_event(&mut self) -> Option<HookEvent> {
        self.ensure_fd().ok()?;

        loop {
            // Drain complete lines from the buffer first.
            if let Some(event) = self.try_parse_line() {
                return Some(event);
            }

            // Read more data from the pipe via non-blocking I/O.
            let afd = self.async_fd.as_ref()?;
            let mut guard = match afd.readable().await {
                Ok(g) => g,
                Err(_) => return None,
            };
            let mut buf = [0u8; 4096];
            match guard.try_io(|inner| {
                nix::unistd::read(inner.as_raw_fd(), &mut buf)
                    .map_err(|e| std::io::Error::from_raw_os_error(e as i32))
            }) {
                Ok(Ok(0)) => return None, // EOF
                Ok(Ok(n)) => self.line_buf.extend_from_slice(&buf[..n]),
                Ok(Err(_)) => return None,
                Err(_would_block) => continue,
            }
        }
    }

    /// Try to extract a parsed event from complete lines in the buffer.
    ///
    /// Drains malformed lines and returns the first valid event, or `None`
    /// if no complete lines remain.
    fn try_parse_line(&mut self) -> Option<HookEvent> {
        loop {
            let pos = self.line_buf.iter().position(|&b| b == b'\n')?;
            let line = String::from_utf8_lossy(&self.line_buf[..pos]).to_string();
            self.line_buf.drain(..=pos);
            if let Some(event) = parse_hook_line(line.trim()) {
                return Some(event);
            }
            // Malformed line — drain it and try the next one.
        }
    }

    /// Ensure the pipe fd is open and registered with tokio.
    ///
    /// Opens with `O_RDWR | O_NONBLOCK`: `O_RDWR` prevents spurious EOF
    /// when the last writer closes; `O_NONBLOCK` enables event-driven reads
    /// through [`AsyncFd`].
    fn ensure_fd(&mut self) -> anyhow::Result<()> {
        if self.async_fd.is_none() {
            let std_file =
                std::fs::OpenOptions::new().read(true).write(true).open(&self.pipe_path)?;
            crate::pty::nbio::set_nonblocking(&std_file)?;
            let owned: OwnedFd = std_file.into();
            let fifo_fd = FifoFd(owned);
            let async_fd = AsyncFd::new(fifo_fd)?;
            self.async_fd = Some(async_fd);
        }
        Ok(())
    }
}

impl Drop for HookReceiver {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.pipe_path);
    }
}

/// Parse a raw JSON line from the hook pipe into a [`HookEvent`].
fn parse_hook_line(line: &str) -> Option<HookEvent> {
    let raw: RawHookEvent = serde_json::from_str(line).ok()?;
    match raw.event.as_str() {
        "post_tool_use" | "after_tool" => {
            let tool = raw
                .data
                .as_ref()
                .and_then(|d| d.get("tool_name"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            Some(HookEvent::ToolComplete { tool })
        }
        "before_agent" => Some(HookEvent::AgentStart),
        "stop" => Some(HookEvent::AgentStop),
        "session_end" => Some(HookEvent::SessionEnd),
        "notification" => {
            let data = raw.data?;
            let notification_type =
                data.get("notification_type").and_then(|v| v.as_str())?.to_string();
            Some(HookEvent::Notification { notification_type })
        }
        "pre_tool_use" => {
            let data = raw.data?;
            let tool =
                data.get("tool_name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let tool_input = data.get("tool_input").cloned();
            Some(HookEvent::PreToolUse { tool, tool_input })
        }
        "user_prompt_submit" => Some(HookEvent::UserPromptSubmit),
        "start" => Some(HookEvent::SessionStart),
        _ => None,
    }
}

#[cfg(test)]
#[path = "hook_recv_tests.rs"]
mod tests;
