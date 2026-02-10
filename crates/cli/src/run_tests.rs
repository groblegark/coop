// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::path::Path;

use serde_json::json;

use crate::driver::claude::setup as claude_setup;
use crate::driver::gemini::setup as gemini_setup;

// -- Claude pristine --

#[test]
fn pristine_claude_no_settings_returns_session_id_and_coop_url() -> anyhow::Result<()> {
    let dir = Path::new("/tmp/test-pristine");
    let setup = claude_setup::prepare(dir, "http://127.0.0.1:8080", None, None, true, None)?;

    // --session-id <uuid>
    assert_eq!(setup.extra_args.len(), 2);
    assert_eq!(setup.extra_args[0], "--session-id");
    assert!(!setup.extra_args[1].is_empty());

    // Only COOP_URL (no COOP_HOOK_PIPE)
    assert_eq!(setup.env_vars.len(), 1);
    assert_eq!(setup.env_vars[0].0, "COOP_URL");
    assert_eq!(setup.env_vars[0].1, "http://127.0.0.1:8080");

    // Session log path for Tier 2
    assert!(setup.session_log_path.is_some());

    // No hook pipe in pristine mode
    assert!(setup.hook_pipe_path.is_none());
    Ok(())
}

#[test]
fn pristine_claude_with_settings_writes_file_without_hooks() -> anyhow::Result<()> {
    let dir = Path::new("/tmp/test-pristine");
    let settings = json!({
        "permissions": { "allow": ["Bash"] },
        "env": { "FOO": "bar" }
    });
    let setup =
        claude_setup::prepare(dir, "http://127.0.0.1:8080", Some(&settings), None, true, None)?;

    // --session-id <uuid> --settings <path>
    assert_eq!(setup.extra_args.len(), 4);
    assert_eq!(setup.extra_args[0], "--session-id");
    assert_eq!(setup.extra_args[2], "--settings");
    let settings_path = Path::new(&setup.extra_args[3]);
    assert!(settings_path.exists());

    // Settings file should NOT contain hooks (no coop hook merge)
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(settings_path)?)?;
    assert!(written.get("hooks").is_none());
    assert_eq!(written["permissions"]["allow"][0], "Bash");

    // No COOP_HOOK_PIPE
    assert!(!setup.env_vars.iter().any(|(k, _)| k == "COOP_HOOK_PIPE"));
    Ok(())
}

#[test]
fn pristine_claude_with_mcp_writes_mcp_config() -> anyhow::Result<()> {
    let dir = Path::new("/tmp/test-pristine");
    let mcp = json!({
        "my-server": { "command": "node", "args": ["server.js"] }
    });
    let setup = claude_setup::prepare(dir, "http://127.0.0.1:8080", None, Some(&mcp), true, None)?;

    // --session-id <uuid> --mcp-config <path>
    assert_eq!(setup.extra_args.len(), 4);
    assert_eq!(setup.extra_args[2], "--mcp-config");
    let mcp_path = Path::new(&setup.extra_args[3]);
    assert!(mcp_path.exists());

    let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(mcp_path)?)?;
    assert!(written.get("mcpServers").is_some());
    assert!(written["mcpServers"]["my-server"]["command"].as_str() == Some("node"));
    Ok(())
}

// -- Gemini pristine --

#[test]
fn pristine_gemini_with_settings_and_mcp() -> anyhow::Result<()> {
    let settings = json!({ "theme": "dark" });
    let mcp = json!({
        "tool-server": { "command": "python", "args": ["serve.py"] }
    });
    let setup = gemini_setup::prepare("http://127.0.0.1:8080", Some(&settings), Some(&mcp), true)?;

    // No CLI args for Gemini
    assert!(setup.extra_args.is_empty());

    // COOP_URL + GEMINI_CLI_SYSTEM_SETTINGS_PATH
    assert_eq!(setup.env_vars.len(), 2);
    assert_eq!(setup.env_vars[0].0, "COOP_URL");
    let settings_env = setup.env_vars.iter().find(|(k, _)| k == "GEMINI_CLI_SYSTEM_SETTINGS_PATH");
    assert!(settings_env.is_some());

    // Settings file has MCP embedded and no hooks
    let settings_val = settings_env.map(|(_, v)| v.clone()).unwrap_or_default();
    let settings_path = Path::new(&settings_val);
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(settings_path)?)?;
    assert_eq!(written["theme"], "dark");
    assert!(written["mcpServers"]["tool-server"]["command"].as_str() == Some("python"));
    assert!(written.get("hooks").is_none());

    // No session log path for Gemini
    assert!(setup.session_log_path.is_none());

    // No hook pipe in pristine mode
    assert!(setup.hook_pipe_path.is_none());
    Ok(())
}
