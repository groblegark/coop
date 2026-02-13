// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::path::Path;

use serde_json::json;
use serial_test::serial;

use super::{project_dir_name, write_credentials_file};

#[test]
fn project_dir_name_keeps_leading_dash() {
    let name = project_dir_name(Path::new("/Users/alice/projects/myapp"));
    assert!(name.starts_with('-'));
    assert_eq!(name, "-Users-alice-projects-myapp");
}

#[test]
fn project_dir_name_replaces_slashes_and_dots() {
    let name = project_dir_name(Path::new("/a/b.c/d"));
    assert!(!name.contains('/'));
    assert!(!name.contains('.'));
    assert_eq!(name, "-a-b-c-d");
}

#[test]
fn prepare_session_creates_settings_file() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = super::prepare(work_dir.path(), "http://127.0.0.1:0", None, None, false, None)?;

    let settings_arg_idx = setup
        .extra_args
        .iter()
        .position(|a| a == "--settings")
        .ok_or_else(|| anyhow::anyhow!("no --settings arg"))?;
    let settings_path = Path::new(&setup.extra_args[settings_arg_idx + 1]);
    assert!(settings_path.exists());

    let content = std::fs::read_to_string(settings_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;
    assert!(parsed.get("hooks").is_some());
    Ok(())
}

#[test]
fn prepare_session_has_session_id_arg() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = super::prepare(work_dir.path(), "http://127.0.0.1:0", None, None, false, None)?;

    assert!(setup.extra_args.contains(&"--session-id".to_owned()));
    let id_idx = setup
        .extra_args
        .iter()
        .position(|a| a == "--session-id")
        .ok_or_else(|| anyhow::anyhow!("no --session-id arg"))?;
    let id = &setup.extra_args[id_idx + 1];
    assert_eq!(id.len(), 36);
    Ok(())
}

#[test]
fn prepare_session_has_env_vars() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = super::prepare(work_dir.path(), "http://127.0.0.1:0", None, None, false, None)?;

    assert!(setup.env_vars.iter().any(|(k, _)| k == "COOP_HOOK_PIPE"));
    Ok(())
}

#[test]
fn prepare_session_pipe_path_in_session_dir() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = super::prepare(work_dir.path(), "http://127.0.0.1:0", None, None, false, None)?;

    let pipe =
        setup.hook_pipe_path.as_ref().ok_or_else(|| anyhow::anyhow!("expected hook_pipe_path"))?;
    assert_eq!(pipe.file_name().and_then(|n| n.to_str()), Some("hook.pipe"));
    Ok(())
}

#[test]
fn prepare_session_with_base_settings_merges_hooks() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let orchestrator = json!({
        "hooks": {
            "SessionStart": [{"matcher": "", "hooks": [{"type": "command", "command": "gt-prime"}]}],
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "gt-guard"}]}]
        },
        "permissions": { "allow": ["Bash", "Read"] }
    });
    let setup = super::prepare(
        work_dir.path(),
        "http://127.0.0.1:0",
        Some(&orchestrator),
        None,
        false,
        None,
    )?;

    let settings_arg_idx = setup
        .extra_args
        .iter()
        .position(|a| a == "--settings")
        .ok_or_else(|| anyhow::anyhow!("no --settings arg"))?;
    let settings_path = Path::new(&setup.extra_args[settings_arg_idx + 1]);
    let content = std::fs::read_to_string(settings_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;

    // SessionStart: orchestrator entry first, coop entry second
    let session_start = parsed["hooks"]["SessionStart"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no SessionStart hooks"))?;
    assert!(session_start.len() >= 2);
    assert_eq!(session_start[0]["hooks"][0]["command"], "gt-prime");

    // Orchestrator permissions pass through, coop send rule appended
    let allow = parsed["permissions"]["allow"].as_array().unwrap();
    assert_eq!(allow[0], "Bash");
    assert_eq!(allow[1], "Read");
    assert!(allow.contains(&json!("Bash(coop send:*)")));

    // Coop-only hook types present
    assert!(parsed["hooks"]["PostToolUse"].as_array().is_some());
    assert!(parsed["hooks"]["Stop"].as_array().is_some());
    Ok(())
}

#[test]
fn prepare_session_injects_coop_send_permission() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = super::prepare(work_dir.path(), "http://127.0.0.1:0", None, None, false, None)?;

    let settings_arg_idx = setup
        .extra_args
        .iter()
        .position(|a| a == "--settings")
        .ok_or_else(|| anyhow::anyhow!("no --settings arg"))?;
    let settings_path = Path::new(&setup.extra_args[settings_arg_idx + 1]);
    let content = std::fs::read_to_string(settings_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;

    let allow = parsed["permissions"]["allow"].as_array().unwrap();
    assert!(allow.contains(&json!("Bash(coop send:*)")));
    Ok(())
}

#[test]
fn prepare_session_with_mcp_writes_config() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let mcp = json!({
        "my-server": { "command": "node", "args": ["server.js"] }
    });
    let setup =
        super::prepare(work_dir.path(), "http://127.0.0.1:0", None, Some(&mcp), false, None)?;

    let mcp_idx = setup
        .extra_args
        .iter()
        .position(|a| a == "--mcp-config")
        .ok_or_else(|| anyhow::anyhow!("no --mcp-config arg"))?;
    let mcp_path = Path::new(&setup.extra_args[mcp_idx + 1]);
    assert!(mcp_path.exists());

    let content = std::fs::read_to_string(mcp_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;
    assert_eq!(parsed["mcpServers"]["my-server"]["command"], "node");
    Ok(())
}

#[test]
#[serial(claude_config_dir)]
fn write_credentials_creates_new_file() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    // Point CLAUDE_CONFIG_DIR to temp dir so we don't touch real config.
    std::env::set_var("CLAUDE_CONFIG_DIR", tmp.path());

    let path = write_credentials_file("sk-ant-test-token")?;
    assert!(path.exists());

    let content = std::fs::read_to_string(&path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;
    assert_eq!(parsed["claudeAiOauth"]["accessToken"], "sk-ant-test-token");
    assert!(parsed["claudeAiOauth"]["refreshToken"].is_null());
    assert_eq!(parsed["claudeAiOauth"]["scopes"][0], "user:inference");

    std::env::remove_var("CLAUDE_CONFIG_DIR");
    Ok(())
}

#[test]
#[serial(claude_config_dir)]
fn write_credentials_preserves_existing_fields() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    std::env::set_var("CLAUDE_CONFIG_DIR", tmp.path());

    // Write an existing credentials file with extra fields.
    let existing = json!({
        "claudeAiOauth": {
            "accessToken": "old-token",
            "refreshToken": "rt-keep-this",
            "expiresAt": 1234567890000_i64,
            "scopes": ["user:inference", "user:sessions:claude_code"],
            "subscriptionType": "max",
            "rateLimitTier": "default_claude_max_20x"
        }
    });
    let cred_path = tmp.path().join(".credentials.json");
    std::fs::write(&cred_path, serde_json::to_string_pretty(&existing)?)?;

    // Write new token — should update accessToken but keep everything else.
    write_credentials_file("sk-ant-new-token")?;

    let content = std::fs::read_to_string(&cred_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;
    assert_eq!(parsed["claudeAiOauth"]["accessToken"], "sk-ant-new-token");
    assert_eq!(parsed["claudeAiOauth"]["refreshToken"], "rt-keep-this");
    assert_eq!(parsed["claudeAiOauth"]["subscriptionType"], "max");
    assert_eq!(parsed["claudeAiOauth"]["scopes"][1], "user:sessions:claude_code");

    std::env::remove_var("CLAUDE_CONFIG_DIR");
    Ok(())
}
