// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

// ── stamp_request_id ──────────────────────────────────────────────────

#[test]
fn stamp_inserts_request_id_when_absent() -> anyhow::Result<()> {
    let mut counter = 0;
    let msg = r#"{"event":"replay:get","offset":0}"#;
    let stamped = stamp_request_id(&mut counter, msg);

    assert_eq!(stamped.request_id, "1");
    let parsed: serde_json::Value = serde_json::from_str(&stamped.text)?;
    assert_eq!(parsed["request_id"], "1");
    assert_eq!(parsed["event"], "replay:get");
    Ok(())
}

#[test]
fn stamp_replaces_existing_request_id() -> anyhow::Result<()> {
    let mut counter = 0;
    let msg = r#"{"event":"agent:get","request_id":"r5"}"#;
    let stamped = stamp_request_id(&mut counter, msg);

    assert_eq!(stamped.request_id, "1");
    let parsed: serde_json::Value = serde_json::from_str(&stamped.text)?;
    assert_eq!(parsed["request_id"], "1");
    // Original client request_id is replaced, not duplicated.
    let obj = parsed.as_object().unwrap();
    assert_eq!(obj.iter().filter(|(k, _)| k.as_str() == "request_id").count(), 1);
    Ok(())
}

#[test]
fn stamp_increments_counter() -> anyhow::Result<()> {
    let mut counter = 0;
    let _ = stamp_request_id(&mut counter, r#"{"event":"a"}"#);
    let second = stamp_request_id(&mut counter, r#"{"event":"b"}"#);
    assert_eq!(second.request_id, "2");
    Ok(())
}

// ── strip_request_id ──────────────────────────────────────────────────

#[test]
fn strip_removes_request_id() -> anyhow::Result<()> {
    let json = r#"{"event":"replay","data":"abc","request_id":"1"}"#;
    let stripped = strip_request_id(json);
    let parsed: serde_json::Value = serde_json::from_str(&stripped)?;
    assert!(parsed.get("request_id").is_none());
    assert_eq!(parsed["event"], "replay");
    assert_eq!(parsed["data"], "abc");
    Ok(())
}

#[test]
fn strip_noop_when_no_request_id() -> anyhow::Result<()> {
    let json = r#"{"event":"pty","data":"xyz"}"#;
    let stripped = strip_request_id(json);
    let parsed: serde_json::Value = serde_json::from_str(&stripped)?;
    assert!(parsed.get("request_id").is_none());
    assert_eq!(parsed["event"], "pty");
    Ok(())
}

// ── extract_route_info ────────────────────────────────────────────────

#[test]
fn route_info_extracts_both_fields() {
    let json = r#"{"event":"replay","request_id":"42"}"#;
    let info = extract_route_info(json);
    assert_eq!(info.event, Some("replay"));
    assert_eq!(info.request_id, Some("42"));
}

#[test]
fn route_info_handles_missing_fields() {
    let json = r#"{"event":"pty"}"#;
    let info = extract_route_info(json);
    assert_eq!(info.event, Some("pty"));
    assert!(info.request_id.is_none());
}

#[test]
fn route_info_handles_invalid_json() {
    let info = extract_route_info("not json");
    assert!(info.event.is_none());
    assert!(info.request_id.is_none());
}

// ── stamp + strip round-trip ──────────────────────────────────────────

#[test]
fn stamp_then_strip_round_trips() -> anyhow::Result<()> {
    let mut counter = 0;
    let original = r#"{"event":"replay:get","offset":0}"#;

    let stamped = stamp_request_id(&mut counter, original);
    // Verify stamp added request_id
    let with_rid: serde_json::Value = serde_json::from_str(&stamped.text)?;
    assert!(with_rid.get("request_id").is_some());

    // Simulate upstream response (same JSON with request_id preserved)
    let response = r#"{"event":"replay","data":"abc","request_id":"1"}"#;
    let stripped = strip_request_id(response);
    let parsed: serde_json::Value = serde_json::from_str(&stripped)?;
    assert!(parsed.get("request_id").is_none());
    assert_eq!(parsed["event"], "replay");
    assert_eq!(parsed["data"], "abc");
    Ok(())
}

// ── client_had_rid detection ──────────────────────────────────────────

#[test]
fn detect_fire_and_forget_message() {
    // Client sends replay:get without request_id (fire-and-forget)
    let msg = r#"{"event":"replay:get","offset":0}"#;
    let had_rid = extract_route_info(msg).request_id.is_some();
    assert!(!had_rid, "fire-and-forget should not have request_id");
}

#[test]
fn detect_rpc_message() {
    // Client sends agent:get with request_id (RPC)
    let msg = r#"{"event":"agent:get","request_id":"r1"}"#;
    let had_rid = extract_route_info(msg).request_id.is_some();
    assert!(had_rid, "RPC message should have request_id");
}

// ── subscription flags ────────────────────────────────────────────────

#[test]
fn subscription_flags_replay_requires_pty() {
    let flags = SubscriptionFlags { pty: false, screen: false, state: false };
    assert!(!flags.matches_event(Some("replay")));

    let flags = SubscriptionFlags { pty: true, screen: false, state: false };
    assert!(flags.matches_event(Some("replay")));
}

// ── build_ws_url ──────────────────────────────────────────────────────

#[test]
fn build_ws_url_http_to_ws() {
    let url = build_ws_url("http://localhost:3000", None, "pty,state");
    assert_eq!(url, "ws://localhost:3000/ws?subscribe=pty,state");
}

#[test]
fn build_ws_url_https_to_wss() {
    let url = build_ws_url("https://example.com", Some("tok"), "pty");
    assert_eq!(url, "wss://example.com/ws?subscribe=pty&token=tok");
}
