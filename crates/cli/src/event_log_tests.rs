// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::driver::AgentState;
use crate::event::{RawHookEvent, TransitionEvent};

use super::EventLog;

#[test]
fn push_and_catchup_state() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let log = EventLog::new(Some(tmp.path()));

    let events = vec![
        TransitionEvent {
            prev: AgentState::Starting,
            next: AgentState::Working,
            seq: 1,
            cause: "hook".into(),
            last_message: None,
        },
        TransitionEvent {
            prev: AgentState::Working,
            next: AgentState::Idle,
            seq: 2,
            cause: "hook".into(),
            last_message: Some("hello".into()),
        },
        TransitionEvent {
            prev: AgentState::Idle,
            next: AgentState::Working,
            seq: 3,
            cause: "nudge".into(),
            last_message: None,
        },
    ];

    for e in &events {
        log.push_transition(e);
    }

    // Catchup from seq 1 should return seq 2 and 3.
    let caught = log.catchup_state(1);
    assert_eq!(caught.len(), 2);
    assert_eq!(caught[0].seq, 2);
    assert_eq!(caught[1].seq, 3);
    assert_eq!(caught[0].last_message.as_deref(), Some("hello"));
    Ok(())
}

#[test]
fn push_and_catchup_hooks() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let log = EventLog::new(Some(tmp.path()));

    log.push_hook(&RawHookEvent { json: serde_json::json!({"event": "a"}) });
    log.push_hook(&RawHookEvent { json: serde_json::json!({"event": "b"}) });

    let caught = log.catchup_hooks(0);
    // hook_seq starts at 0 and we filter > since, so hook_seq=0 is not included
    // when since_hook_seq=0.
    assert_eq!(caught.len(), 1);
    assert_eq!(caught[0].hook_seq, 1);
    assert_eq!(caught[0].json["event"], "b");
    Ok(())
}

#[test]
fn catchup_empty_when_no_events() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let log = EventLog::new(Some(tmp.path()));

    assert!(log.catchup_state(0).is_empty());
    assert!(log.catchup_hooks(0).is_empty());
    Ok(())
}

#[test]
fn catchup_with_no_session_dir() {
    let log = EventLog::new(None);

    // Push should be no-op, catchup returns empty.
    log.push_transition(&TransitionEvent {
        prev: AgentState::Starting,
        next: AgentState::Working,
        seq: 1,
        cause: String::new(),
        last_message: None,
    });
    log.push_hook(&RawHookEvent { json: serde_json::json!({}) });

    assert!(log.catchup_state(0).is_empty());
    assert!(log.catchup_hooks(0).is_empty());
}
