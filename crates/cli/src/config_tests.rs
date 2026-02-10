// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::time::Duration;

use clap::Parser;

use super::{AgentType, Config};

fn parse(args: &[&str]) -> Config {
    Config::parse_from(args)
}

#[test]
fn valid_config_with_port_and_command() -> anyhow::Result<()> {
    let config = parse(&["coop", "--port", "8080", "--", "echo", "hello"]);
    config.validate()?;
    assert_eq!(config.port, Some(8080));
    assert_eq!(config.command, vec!["echo", "hello"]);
    Ok(())
}

#[test]
fn valid_config_with_socket_and_command() -> anyhow::Result<()> {
    let config = parse(&["coop", "--socket", "/tmp/coop.sock", "--", "bash"]);
    config.validate()?;
    assert_eq!(config.socket.as_deref(), Some("/tmp/coop.sock"));
    Ok(())
}

#[test]
fn valid_config_with_attach() -> anyhow::Result<()> {
    let config = parse(&["coop", "--port", "8080", "--attach", "tmux:my-session"]);
    config.validate()?;
    assert_eq!(config.attach.as_deref(), Some("tmux:my-session"));
    Ok(())
}

#[yare::parameterized(
    no_transport        = { &["coop", "--", "echo"], "--port or --socket" },
    no_command          = { &["coop", "--port", "8080"], "command or --attach" },
    both_cmd_and_attach = { &["coop", "--port", "8080", "--attach", "tmux:sess", "--", "echo"],
                            "cannot specify both" },
)]
fn invalid_config(args: &[&str], expected_substr: &str) {
    let config = parse(args);
    crate::assert_err_contains!(config.validate(), expected_substr);
}

#[test]
fn agent_claude() -> anyhow::Result<()> {
    let config = parse(&["coop", "--port", "8080", "--agent", "claude", "--", "echo"]);
    assert_eq!(config.agent_enum()?, AgentType::Claude);
    Ok(())
}

#[test]
fn agent_unknown_default() -> anyhow::Result<()> {
    let config = parse(&["coop", "--port", "8080", "--", "echo"]);
    assert_eq!(config.agent_enum()?, AgentType::Unknown);
    Ok(())
}

#[test]
fn agent_invalid() {
    let config = parse(&["coop", "--port", "8080", "--agent", "gpt", "--", "echo"]);
    assert!(config.agent_enum().is_err());
}

#[test]
fn defaults_are_correct() {
    let config = parse(&["coop", "--port", "8080", "--", "echo"]);
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.cols, 200);
    assert_eq!(config.rows, 50);
    assert_eq!(config.ring_size, 1048576);
    assert_eq!(config.log_format, "json");
    assert_eq!(config.log_level, "info");
}

#[test]
fn env_duration_defaults() {
    // These read env vars, so with no env set we get production defaults.
    let config = parse(&["coop", "--port", "8080", "--", "echo"]);
    assert_eq!(config.shutdown_timeout(), Duration::from_secs(10));
    assert_eq!(config.screen_debounce(), Duration::from_millis(50));
    assert_eq!(config.process_poll(), Duration::from_secs(5));
    assert_eq!(config.screen_poll(), Duration::from_secs(2));
    assert_eq!(config.log_poll(), Duration::from_secs(5));
    assert_eq!(config.tmux_poll(), Duration::from_secs(1));
    assert_eq!(config.pty_reap(), Duration::from_millis(50));
    assert_eq!(config.keyboard_delay(), Duration::from_millis(200));
    assert_eq!(config.idle_timeout(), Duration::ZERO);
}
