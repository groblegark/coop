// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::path::Path;

use super::{prepare_claude_session, project_dir_name};

#[test]
fn project_dir_name_strips_leading_separator() {
    let name = project_dir_name(Path::new("/Users/alice/projects/myapp"));
    assert!(!name.starts_with('-'));
    assert!(name.contains("Users-alice-projects-myapp"));
}

#[test]
fn project_dir_name_replaces_slashes() {
    let name = project_dir_name(Path::new("/a/b/c"));
    assert!(!name.contains('/'));
}

#[test]
fn prepare_session_creates_settings_file() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_claude_session(work_dir.path())?;

    // Settings file should exist in the temp dir
    let settings_arg_idx = setup
        .extra_args
        .iter()
        .position(|a| a == "--settings")
        .ok_or_else(|| anyhow::anyhow!("no --settings arg"))?;
    let settings_path = Path::new(&setup.extra_args[settings_arg_idx + 1]);
    assert!(settings_path.exists());

    // Settings should contain hook config
    let content = std::fs::read_to_string(settings_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;
    assert!(parsed.get("hooks").is_some());
    Ok(())
}

#[test]
fn prepare_session_has_session_id_arg() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_claude_session(work_dir.path())?;

    assert!(setup.extra_args.contains(&"--session-id".to_owned()));
    // Session ID should be a UUID (36 chars with hyphens)
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
    let setup = prepare_claude_session(work_dir.path())?;

    assert!(setup.env_vars.iter().any(|(k, _)| k == "COOP_HOOK_PIPE"));
    Ok(())
}

#[test]
fn prepare_session_pipe_path_in_temp_dir() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_claude_session(work_dir.path())?;

    assert!(setup.hook_pipe_path.file_name().is_some());
    assert_eq!(
        setup.hook_pipe_path.file_name().and_then(|n| n.to_str()),
        Some("hook.pipe")
    );
    Ok(())
}
