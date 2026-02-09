// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::path::Path;

use serde_json::{json, Value};

/// Generate the Gemini CLI hook configuration JSON.
///
/// Gemini hooks receive JSON on stdin and must output JSON on stdout.
/// The hooks read stdin, wrap it, and write to the named pipe at `$COOP_HOOK_PIPE`:
/// - `BeforeTool`: fires before each tool call for permission detection
/// - `AfterTool`: fires after each tool call, includes tool name and result
/// - `AfterAgent`: fires when the agent wants to stop; curls gating endpoint
/// - `SessionEnd`: fires when the session ends
/// - `Notification`: fires on system notifications (e.g. `ToolPermission`)
pub fn generate_hook_config(pipe_path: &Path) -> Value {
    // Use $COOP_HOOK_PIPE so the config is portable across processes.
    // The actual path is passed via environment variable.
    let _ = pipe_path; // validated by caller; config uses env var

    // AfterAgent hook: write stop event to pipe, then curl gating endpoint.
    // Gemini uses {"continue":true} to prevent stopping (vs Claude's {"decision":"block"}).
    // If curl fails (coop not ready), the hook outputs nothing → agent proceeds.
    let after_agent_command = concat!(
        "input=$(cat); ",
        "printf '{\"event\":\"stop\",\"data\":%s}\\n' \"$input\" > \"$COOP_HOOK_PIPE\"; ",
        "response=$(printf '%s' \"$input\" | curl -sf -X POST ",
        "-H 'Content-Type: application/json' ",
        "-d @- \"$COOP_URL/api/v1/hooks/stop\" 2>/dev/null); ",
        "if printf '%s' \"$response\" | grep -q '\"block\"'; then printf '{\"continue\":true}'; fi"
    );

    json!({
        "hooks": {
            "BeforeTool": [{
                "matcher": "",
                "hooks": [{
                    "type": "command",
                    "command": "input=$(cat); printf '{\"event\":\"pre_tool_use\",\"data\":%s}\\n' \"$input\" > \"$COOP_HOOK_PIPE\""
                }]
            }],
            "AfterTool": [{
                "matcher": "",
                "hooks": [{
                    "type": "command",
                    "command": "input=$(cat); printf '{\"event\":\"after_tool\",\"data\":%s}\\n' \"$input\" > \"$COOP_HOOK_PIPE\""
                }]
            }],
            "AfterAgent": [{
                "matcher": "",
                "hooks": [{
                    "type": "command",
                    "command": after_agent_command
                }]
            }],
            "SessionEnd": [{
                "matcher": "",
                "hooks": [{
                    "type": "command",
                    "command": "cat > /dev/null; echo '{\"event\":\"session_end\"}' > \"$COOP_HOOK_PIPE\""
                }]
            }],
            "Notification": [{
                "matcher": "",
                "hooks": [{
                    "type": "command",
                    "command": "input=$(cat); printf '{\"event\":\"notification\",\"data\":%s}\\n' \"$input\" > \"$COOP_HOOK_PIPE\""
                }]
            }]
        }
    })
}

/// Return environment variables to set on the Gemini child process.
pub fn hook_env_vars(pipe_path: &Path, coop_url: &str) -> Vec<(String, String)> {
    vec![
        (
            "COOP_HOOK_PIPE".to_string(),
            pipe_path.display().to_string(),
        ),
        ("COOP_URL".to_string(), coop_url.to_string()),
    ]
}

/// Write the hook config to a settings file and return its path.
///
/// The config file is written into `config_dir` so Gemini can load it
/// via `GEMINI_CLI_SYSTEM_SETTINGS_PATH`.
pub fn write_hook_config(
    config_dir: &Path,
    pipe_path: &Path,
) -> anyhow::Result<std::path::PathBuf> {
    let config = generate_hook_config(pipe_path);
    let config_path = config_dir.join("coop-gemini-settings.json");
    let contents = serde_json::to_string_pretty(&config)?;
    std::fs::write(&config_path, contents)?;
    Ok(config_path)
}

#[cfg(test)]
#[path = "hooks_tests.rs"]
mod tests;
