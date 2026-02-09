// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::prepare_gemini_session;

#[test]
fn prepare_session_creates_settings_file() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_gemini_session(work_dir.path(), "http://127.0.0.1:0")?;

    // Settings file should exist in the temp dir (pointed to by env var)
    let settings_path = setup
        .env_vars
        .iter()
        .find(|(k, _)| k == "GEMINI_CLI_SYSTEM_SETTINGS_PATH")
        .map(|(_, v)| std::path::PathBuf::from(v))
        .ok_or_else(|| anyhow::anyhow!("no GEMINI_CLI_SYSTEM_SETTINGS_PATH env var"))?;
    assert!(settings_path.exists());

    // Settings should contain hook config
    let content = std::fs::read_to_string(&settings_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;
    assert!(parsed.get("hooks").is_some());
    Ok(())
}

#[test]
fn prepare_session_has_env_vars() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_gemini_session(work_dir.path(), "http://127.0.0.1:0")?;

    assert!(setup.env_vars.iter().any(|(k, _)| k == "COOP_HOOK_PIPE"));
    assert!(setup.env_vars.iter().any(|(k, _)| k == "COOP_URL"));
    assert!(setup
        .env_vars
        .iter()
        .any(|(k, _)| k == "GEMINI_CLI_SYSTEM_SETTINGS_PATH"));
    Ok(())
}

#[test]
fn prepare_session_pipe_path_in_temp_dir() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_gemini_session(work_dir.path(), "http://127.0.0.1:0")?;

    assert!(setup.hook_pipe_path.file_name().is_some());
    assert_eq!(
        setup.hook_pipe_path.file_name().and_then(|n| n.to_str()),
        Some("hook.pipe")
    );
    Ok(())
}

#[test]
fn prepare_session_has_no_extra_args() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_gemini_session(work_dir.path(), "http://127.0.0.1:0")?;

    // Gemini doesn't need extra CLI args (no --session-id, etc.)
    assert!(setup.extra_args.is_empty());
    Ok(())
}
