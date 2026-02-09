// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

#[test]
fn missing_coop_url_returns_2() {
    std::env::remove_var("COOP_URL");
    assert_eq!(run(&[]), 2);
}

#[test]
fn help_flag_returns_0() {
    assert_eq!(run(&["--help".to_string()]), 0);
}

#[test]
fn help_short_flag_returns_0() {
    assert_eq!(run(&["-h".to_string()]), 0);
}

#[test]
fn connection_refused_returns_1() {
    assert_eq!(run(&["http://127.0.0.1:1".to_string()]), 1);
}

// -- StatuslineConfig tests ------------------------------------------------

#[test]
fn statusline_defaults_enabled_builtin() {
    let cfg = StatuslineConfig::from_args(&[]);
    assert!(cfg.enabled);
    assert!(cfg.cmd.is_none());
    assert_eq!(cfg.interval, Duration::from_secs(DEFAULT_STATUSLINE_INTERVAL));
}

#[test]
fn statusline_no_statusline_flag() {
    let cfg = StatuslineConfig::from_args(&["--no-statusline".to_string()]);
    assert!(!cfg.enabled);
}

#[test]
fn statusline_cmd_space_separated() {
    let cfg = StatuslineConfig::from_args(&[
        "--statusline-cmd".to_string(),
        "echo hello".to_string(),
    ]);
    assert_eq!(cfg.cmd.as_deref(), Some("echo hello"));
}

#[test]
fn statusline_cmd_equals_syntax() {
    let cfg = StatuslineConfig::from_args(&["--statusline-cmd=echo hello".to_string()]);
    assert_eq!(cfg.cmd.as_deref(), Some("echo hello"));
}

#[test]
fn statusline_interval_override() {
    let cfg = StatuslineConfig::from_args(&[
        "--statusline-interval".to_string(),
        "10".to_string(),
    ]);
    assert_eq!(cfg.interval, Duration::from_secs(10));
}

#[test]
fn statusline_interval_equals_syntax() {
    let cfg = StatuslineConfig::from_args(&["--statusline-interval=3".to_string()]);
    assert_eq!(cfg.interval, Duration::from_secs(3));
}

// -- builtin_statusline tests ---------------------------------------------

#[test]
fn builtin_statusline_format() {
    let state = AttachState {
        agent_state: "working".to_owned(),
        cols: 120,
        rows: 40,
        started: Instant::now(),
    };
    let line = builtin_statusline(&state);
    assert!(line.contains("[coop]"));
    assert!(line.contains("working"));
    assert!(line.contains("120x40"));
}

// -- run_statusline_cmd tests ---------------------------------------------

#[test]
fn run_statusline_cmd_captures_output() {
    let state = AttachState::new(80, 24);
    let result = run_statusline_cmd("echo test-output", &state);
    assert_eq!(result, "test-output");
}

#[test]
fn run_statusline_cmd_expands_state() {
    let mut state = AttachState::new(80, 24);
    state.agent_state = "idle".to_owned();
    let result = run_statusline_cmd("echo {state}", &state);
    assert_eq!(result, "idle");
}

#[test]
fn run_statusline_cmd_expands_dimensions() {
    let state = AttachState::new(120, 40);
    let result = run_statusline_cmd("echo {cols}x{rows}", &state);
    assert_eq!(result, "120x40");
}

#[test]
fn run_statusline_cmd_failed_command() {
    let state = AttachState::new(80, 24);
    let result = run_statusline_cmd("false", &state);
    assert!(result.contains("failed"));
}

// -- find_arg_value tests -------------------------------------------------

#[test]
fn find_arg_value_space_separated() {
    let args = vec!["--key".to_string(), "val".to_string()];
    assert_eq!(find_arg_value(&args, "--key"), Some("val".to_string()));
}

#[test]
fn find_arg_value_equals_syntax() {
    let args = vec!["--key=val".to_string()];
    assert_eq!(find_arg_value(&args, "--key"), Some("val".to_string()));
}

#[test]
fn find_arg_value_not_found() {
    let args = vec!["--other".to_string(), "val".to_string()];
    assert_eq!(find_arg_value(&args, "--key"), None);
}
