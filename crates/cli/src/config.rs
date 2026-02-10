// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::driver::AgentType;
use crate::start::StartConfig;
use crate::stop::StopConfig;

/// Terminal session manager for AI coding agents.
#[derive(Debug, Parser)]
#[command(name = "coop", version, about)]
pub struct Config {
    /// HTTP port to listen on.
    #[arg(long, env = "COOP_PORT")]
    pub port: Option<u16>,

    /// Unix socket path for HTTP.
    #[arg(long, env = "COOP_SOCKET")]
    pub socket: Option<String>,

    /// Host address to bind to.
    #[arg(long, env = "COOP_HOST", default_value = "0.0.0.0")]
    pub host: String,

    /// gRPC port to listen on.
    #[arg(long, env = "COOP_GRPC_PORT")]
    pub port_grpc: Option<u16>,

    /// Bearer token for API authentication.
    #[arg(long, env = "COOP_AUTH_TOKEN")]
    pub auth_token: Option<String>,

    /// Agent type (claude, codex, gemini, unknown).
    #[arg(long, env = "COOP_AGENT", default_value = "unknown")]
    pub agent: String,

    /// Path to agent-specific config file.
    #[arg(long, env = "COOP_AGENT_CONFIG")]
    pub agent_config: Option<PathBuf>,

    /// Attach to an existing session (e.g. tmux:session-name).
    #[arg(long, env = "COOP_ATTACH")]
    pub attach: Option<String>,

    /// Terminal columns.
    #[arg(long, env = "COOP_COLS", default_value = "200")]
    pub cols: u16,

    /// Terminal rows.
    #[arg(long, env = "COOP_ROWS", default_value = "50")]
    pub rows: u16,

    /// Ring buffer size in bytes.
    #[arg(long, env = "COOP_RING_SIZE", default_value = "1048576")]
    pub ring_size: usize,

    /// TERM environment variable for the child process.
    #[arg(long, env = "TERM", default_value = "xterm-256color")]
    pub term: String,

    /// Health-check-only HTTP port.
    #[arg(long, env = "COOP_HEALTH_PORT")]
    pub port_health: Option<u16>,

    /// Log format (json or text).
    #[arg(long, env = "COOP_LOG_FORMAT", default_value = "json")]
    pub log_format: String,

    /// Log level (trace, debug, info, warn, error).
    #[arg(long, env = "COOP_LOG_LEVEL", default_value = "info")]
    pub log_level: String,

    /// Resume a previous session from a log path or workspace ID.
    #[arg(long, env = "COOP_RESUME")]
    pub resume: Option<String>,

    /// Command to run (after --).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

impl Config {
    /// Validate the configuration after parsing.
    pub fn validate(&self) -> anyhow::Result<()> {
        // Must have at least one transport
        if self.port.is_none() && self.socket.is_none() {
            anyhow::bail!("either --port or --socket must be specified");
        }

        // Must have either command or attach (not both, not neither)
        let has_command = !self.command.is_empty();
        let has_attach = self.attach.is_some();

        if has_command && has_attach {
            anyhow::bail!("cannot specify both a command and --attach");
        }
        if !has_command && !has_attach {
            anyhow::bail!("either a command or --attach must be specified");
        }

        // Validate agent type
        self.agent_enum()?;

        // --resume is only valid with --agent claude and cannot combine with --attach
        if self.resume.is_some() {
            if self.agent_enum()? != AgentType::Claude {
                anyhow::bail!("--resume is only supported with --agent claude");
            }
            if self.attach.is_some() {
                anyhow::bail!("--resume cannot be combined with --attach");
            }
        }

        Ok(())
    }

    // -- Env-only tuning knobs (COOP_*_MS) --------------------------------

    /// Time to wait for backend exit before SIGKILL.
    pub fn shutdown_timeout(&self) -> Duration {
        env_duration_ms("COOP_SHUTDOWN_TIMEOUT_MS", 10_000)
    }

    /// Screen debounce interval for broadcasting ScreenUpdate events.
    pub fn screen_debounce(&self) -> Duration {
        env_duration_ms("COOP_SCREEN_DEBOUNCE_MS", 50)
    }

    /// Process monitor poll interval (Tier 4).
    pub fn process_poll(&self) -> Duration {
        env_duration_ms("COOP_PROCESS_POLL_MS", 5_000)
    }

    /// Screen parser poll interval (Tier 5, unknown driver).
    pub fn screen_poll(&self) -> Duration {
        env_duration_ms("COOP_SCREEN_POLL_MS", 2_000)
    }

    /// Claude screen detector: fast poll interval during startup window.
    pub fn screen_startup_poll(&self) -> Duration {
        env_duration_ms("COOP_SCREEN_STARTUP_POLL_MS", 3_000)
    }

    /// Claude screen detector: slow poll interval after startup window.
    pub fn screen_steady_poll(&self) -> Duration {
        env_duration_ms("COOP_SCREEN_STEADY_POLL_MS", 15_000)
    }

    /// Claude screen detector: how long to use the fast startup poll.
    pub fn screen_startup_window(&self) -> Duration {
        env_duration_secs("COOP_SCREEN_STARTUP_WINDOW_SECS", 15)
    }

    /// Log watcher fallback poll interval (Tier 2).
    pub fn log_poll(&self) -> Duration {
        env_duration_ms("COOP_LOG_POLL_MS", 5_000)
    }

    /// Tmux capture-pane poll interval.
    pub fn tmux_poll(&self) -> Duration {
        env_duration_ms("COOP_TMUX_POLL_MS", 1_000)
    }

    /// PTY reap check interval in the NativePty drop handler.
    pub fn pty_reap(&self) -> Duration {
        env_duration_ms("COOP_PTY_REAP_MS", 50)
    }

    /// Minimum gap between keystrokes in multi-step input sequences.
    pub fn keyboard_delay(&self) -> Duration {
        env_duration_ms("COOP_KEYBOARD_DELAY_MS", 200)
    }

    /// Per-byte delay added to the base keyboard delay for long nudge messages.
    pub fn keyboard_delay_per_byte(&self) -> Duration {
        env_duration_ms("COOP_KEYBOARD_DELAY_PER_BYTE_MS", 1)
    }

    /// Maximum nudge delay (caps the base + per-byte scaling).
    pub fn keyboard_delay_max(&self) -> Duration {
        env_duration_ms("COOP_KEYBOARD_DELAY_MAX_MS", 5000)
    }

    /// Timeout before retrying Enter after a nudge delivery.
    /// If the agent doesn't transition to Working within this window,
    /// a single `\r` is re-sent. Set to 0 to disable.
    pub fn nudge_timeout(&self) -> Duration {
        env_duration_ms("COOP_NUDGE_TIMEOUT_MS", 4000)
    }

    /// Idle timeout (0 = disabled).
    pub fn idle_timeout(&self) -> Duration {
        env_duration_secs("COOP_IDLE_TIMEOUT_SECS", 0)
    }

    /// Build a minimal `Config` for tests (port 0, `echo` command).
    #[doc(hidden)]
    pub fn test() -> Self {
        Self {
            port: Some(0),
            socket: None,
            host: "127.0.0.1".into(),
            port_grpc: None,
            auth_token: None,
            agent: "unknown".into(),
            agent_config: None,
            attach: None,
            cols: 80,
            rows: 24,
            ring_size: 4096,
            term: "xterm-256color".into(),
            port_health: None,
            log_format: "json".into(),
            log_level: "debug".into(),
            resume: None,
            command: vec!["echo".into()],
        }
    }

    /// Parse the agent type string into an enum.
    pub fn agent_enum(&self) -> anyhow::Result<AgentType> {
        match self.agent.to_lowercase().as_str() {
            "claude" => Ok(AgentType::Claude),
            "codex" => Ok(AgentType::Codex),
            "gemini" => Ok(AgentType::Gemini),
            "unknown" => Ok(AgentType::Unknown),
            other => anyhow::bail!("invalid agent type: {other}"),
        }
    }
}

/// Contents of the `--agent-config` JSON file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentFileConfig {
    /// Stop hook configuration. `None` means default allow behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<StopConfig>,
    /// Start hook configuration. `None` means no injection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<StartConfig>,
}

/// Load and parse the agent config file at `path`.
///
/// Returns `AgentFileConfig` with any missing keys set to `None`.
pub fn load_agent_config(path: &Path) -> anyhow::Result<AgentFileConfig> {
    let contents = std::fs::read_to_string(path)?;
    let config: AgentFileConfig = serde_json::from_str(&contents)?;
    Ok(config)
}

fn env_duration_secs(var: &str, default: u64) -> Duration {
    let secs = std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    Duration::from_secs(secs)
}

fn env_duration_ms(var: &str, default: u64) -> Duration {
    let ms = std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    Duration::from_millis(ms)
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
