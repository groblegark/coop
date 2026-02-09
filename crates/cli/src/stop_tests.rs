// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::collections::BTreeMap;

use super::*;

#[test]
fn default_stop_config_is_allow() {
    let config = StopConfig::default();
    assert_eq!(config.mode, StopMode::Allow);
    assert!(config.prompt.is_none());
    assert!(config.schema.is_none());
}

#[test]
fn deserialize_allow_mode() -> anyhow::Result<()> {
    let json = r#"{"mode": "allow"}"#;
    let config: StopConfig = serde_json::from_str(json)?;
    assert_eq!(config.mode, StopMode::Allow);
    Ok(())
}

#[test]
fn deserialize_signal_mode_with_prompt() -> anyhow::Result<()> {
    let json = r#"{"mode": "signal", "prompt": "Complete the task before stopping."}"#;
    let config: StopConfig = serde_json::from_str(json)?;
    assert_eq!(config.mode, StopMode::Signal);
    assert_eq!(
        config.prompt.as_deref(),
        Some("Complete the task before stopping.")
    );
    Ok(())
}

#[test]
fn deserialize_signal_mode_with_schema() -> anyhow::Result<()> {
    let json = r#"{
        "mode": "signal",
        "prompt": "Signal when done.",
        "schema": {
            "fields": {
                "status": {
                    "required": true,
                    "enum": ["success", "failure"],
                    "descriptions": {
                        "success": "Task completed successfully",
                        "failure": "Task could not be completed"
                    },
                    "description": "Outcome of the task"
                },
                "notes": {
                    "description": "Optional notes"
                }
            }
        }
    }"#;
    let config: StopConfig = serde_json::from_str(json)?;
    assert_eq!(config.mode, StopMode::Signal);
    let schema = config.schema.as_ref().expect("schema should be present");
    assert_eq!(schema.fields.len(), 2);
    let status = &schema.fields["status"];
    assert!(status.required);
    assert_eq!(status.r#enum.as_ref().map(|v| v.len()), Some(2));
    let notes = &schema.fields["notes"];
    assert!(!notes.required);
    Ok(())
}

#[test]
fn deserialize_empty_object_is_defaults() -> anyhow::Result<()> {
    let json = "{}";
    let config: StopConfig = serde_json::from_str(json)?;
    assert_eq!(config.mode, StopMode::Allow);
    assert!(config.prompt.is_none());
    assert!(config.schema.is_none());
    Ok(())
}

#[test]
fn generate_block_reason_default_no_schema() {
    let config = StopConfig {
        mode: StopMode::Signal,
        prompt: None,
        schema: None,
    };
    let reason =
        generate_block_reason(&config, "http://127.0.0.1:8080/api/v1/hooks/stop/resolve");
    assert!(reason.contains("You must signal before stopping."));
    assert!(reason.contains("-d '{}'"));
    assert!(reason.contains("http://127.0.0.1:8080/api/v1/hooks/stop/resolve"));
    // Should NOT contain the old placeholder
    assert!(!reason.contains(r#"{"your":"json"}"#));
}

#[test]
fn generate_block_reason_custom_prompt_no_schema() {
    let config = StopConfig {
        mode: StopMode::Signal,
        prompt: Some("Finish your work first.".to_owned()),
        schema: None,
    };
    let reason =
        generate_block_reason(&config, "http://localhost:3000/api/v1/hooks/stop/resolve");
    assert!(reason.contains("Finish your work first."));
    assert!(reason.contains("You must signal before stopping."));
    assert!(reason.contains("-d '{}'"));
}

#[test]
fn generate_block_reason_with_enum_schema_expands_commands() {
    let mut fields = BTreeMap::new();
    fields.insert(
        "status".to_owned(),
        StopSchemaField {
            required: true,
            r#enum: Some(vec!["done".to_owned(), "error".to_owned()]),
            descriptions: Some({
                let mut d = BTreeMap::new();
                d.insert("done".to_owned(), "Work completed".to_owned());
                d.insert("error".to_owned(), "Something went wrong".to_owned());
                d
            }),
            description: Some("Task outcome".to_owned()),
        },
    );
    let config = StopConfig {
        mode: StopMode::Signal,
        prompt: Some("Signal when ready.".to_owned()),
        schema: Some(StopSchema { fields }),
    };
    let reason =
        generate_block_reason(&config, "http://127.0.0.1:9000/api/v1/hooks/stop/resolve");

    // Should have one curl per enum value
    assert!(reason.contains("Run one of:"));
    assert!(reason.contains(r#"-d '{"status":"done"}'"#));
    assert!(reason.contains("Work completed"));
    assert!(reason.contains(r#"-d '{"status":"error"}'"#));
    assert!(reason.contains("Something went wrong"));
    // Custom prompt preserved
    assert!(reason.contains("Signal when ready."));
}

#[test]
fn generate_block_reason_enum_with_extra_fields() {
    let mut fields = BTreeMap::new();
    fields.insert(
        "notes".to_owned(),
        StopSchemaField {
            required: false,
            r#enum: None,
            descriptions: None,
            description: Some("Optional notes".to_owned()),
        },
    );
    fields.insert(
        "status".to_owned(),
        StopSchemaField {
            required: true,
            r#enum: Some(vec!["success".to_owned(), "failure".to_owned()]),
            descriptions: Some({
                let mut d = BTreeMap::new();
                d.insert(
                    "success".to_owned(),
                    "Task completed successfully".to_owned(),
                );
                d.insert(
                    "failure".to_owned(),
                    "Task could not be completed".to_owned(),
                );
                d
            }),
            description: Some("Outcome of the task".to_owned()),
        },
    );
    let config = StopConfig {
        mode: StopMode::Signal,
        prompt: None,
        schema: Some(StopSchema { fields }),
    };
    let reason =
        generate_block_reason(&config, "http://127.0.0.1:9000/api/v1/hooks/stop/resolve");

    // Single enum field → expanded with extra fields filled in
    assert!(reason.contains("Run one of:"));
    assert!(reason.contains(r#""status":"success""#));
    assert!(reason.contains(r#""status":"failure""#));
    // Non-enum field should use placeholder
    assert!(reason.contains(r#""notes":"<notes>""#));
}

#[test]
fn generate_block_reason_non_enum_schema() {
    let mut fields = BTreeMap::new();
    fields.insert(
        "message".to_owned(),
        StopSchemaField {
            required: true,
            r#enum: None,
            descriptions: None,
            description: Some("A message".to_owned()),
        },
    );
    let config = StopConfig {
        mode: StopMode::Signal,
        prompt: None,
        schema: Some(StopSchema { fields }),
    };
    let reason =
        generate_block_reason(&config, "http://127.0.0.1:9000/api/v1/hooks/stop/resolve");

    // No enum → single command with placeholder
    assert!(reason.contains("You must signal before stopping."));
    assert!(reason.contains(r#"-d '{"message":"<message>"}'"#));
    assert!(!reason.contains("Run one of:"));
}

#[test]
fn stop_type_as_str() {
    assert_eq!(StopType::Signaled.as_str(), "signaled");
    assert_eq!(StopType::Error.as_str(), "error");
    assert_eq!(StopType::SafetyValve.as_str(), "safety_valve");
    assert_eq!(StopType::Blocked.as_str(), "blocked");
    assert_eq!(StopType::Allowed.as_str(), "allowed");
}

#[test]
fn stop_config_roundtrip_json() -> anyhow::Result<()> {
    let config = StopConfig {
        mode: StopMode::Signal,
        prompt: Some("test prompt".to_owned()),
        schema: None,
    };
    let json = serde_json::to_string(&config)?;
    let parsed: StopConfig = serde_json::from_str(&json)?;
    assert_eq!(parsed.mode, StopMode::Signal);
    assert_eq!(parsed.prompt.as_deref(), Some("test prompt"));
    Ok(())
}

#[test]
fn stop_state_emit_increments_seq() {
    let state = StopState::new(StopConfig::default(), "http://test".to_owned());
    let mut rx = state.stop_tx.subscribe();

    let e1 = state.emit(StopType::Blocked, None, None);
    assert_eq!(e1.seq, 0);
    let e2 = state.emit(StopType::Allowed, None, None);
    assert_eq!(e2.seq, 1);

    // Events should also be received on the broadcast channel.
    let received = rx.try_recv().expect("should receive event");
    assert_eq!(received.seq, 0);
}
