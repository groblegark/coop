// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

/// Configuration for the coopmux proxy.
#[derive(Debug, Clone, clap::Args)]
pub struct MuxConfig {
    /// Host to bind on.
    #[arg(long, default_value = "127.0.0.1", env = "COOP_MUX_HOST")]
    pub host: String,

    /// Port to listen on.
    #[arg(long, default_value_t = 9800, env = "COOP_MUX_PORT")]
    pub port: u16,

    /// Bearer token for downstream API auth. If unset, auth is disabled.
    #[arg(long, env = "COOP_MUX_AUTH_TOKEN")]
    pub auth_token: Option<String>,

    /// Screen poll interval in milliseconds.
    #[arg(long, default_value_t = 1000, env = "COOP_MUX_SCREEN_POLL_MS")]
    pub screen_poll_ms: u64,

    /// Status poll interval in milliseconds.
    #[arg(long, default_value_t = 2000, env = "COOP_MUX_STATUS_POLL_MS")]
    pub status_poll_ms: u64,

    /// Health check interval in milliseconds.
    #[arg(long, default_value_t = 10000, env = "COOP_MUX_HEALTH_CHECK_MS")]
    pub health_check_ms: u64,

    /// Max consecutive health failures before evicting a session.
    #[arg(long, default_value_t = 3, env = "COOP_MUX_MAX_HEALTH_FAILURES")]
    pub max_health_failures: u32,

    /// Launch command template for spawning new sessions (shell command via `sh -c`).
    #[arg(long, env = "COOP_MUX_LAUNCH")]
    pub launch: Option<String>,

    /// Path to credential configuration JSON file.
    #[arg(long, env = "COOP_MUX_CREDENTIAL_CONFIG")]
    pub credential_config: Option<std::path::PathBuf>,

    /// Pre-warm LRU cache capacity (number of sessions to slow-poll).
    #[arg(long, default_value_t = 64, env = "COOP_MUX_PREWARM_CAPACITY")]
    pub prewarm_capacity: usize,

    /// Pre-warm poll interval in milliseconds.
    #[arg(long, default_value_t = 15000, env = "COOP_MUX_PREWARM_POLL_MS")]
    pub prewarm_poll_ms: u64,

    /// Serve web assets from disk instead of embedded (for live reload during dev).
    #[cfg(debug_assertions)]
    #[arg(long, hide = true, env = "COOP_HOT")]
    pub hot: bool,
}

impl MuxConfig {
    pub fn screen_poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.screen_poll_ms)
    }

    pub fn status_poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.status_poll_ms)
    }

    pub fn health_check_interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.health_check_ms)
    }

    pub fn prewarm_poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.prewarm_poll_ms)
    }
}
