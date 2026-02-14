// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use serde_json::json;

use super::*;

#[test]
fn accumulate_single_delta() {
    let mut usage = SessionUsage::default();
    let delta = UsageDelta {
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_input_tokens: 10,
        cache_read_input_tokens: 20,
        cost_usd: 0.005,
        duration_api_ms: 1200,
    };
    usage.accumulate(&delta);
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 50);
    assert_eq!(usage.cache_write_tokens, 10);
    assert_eq!(usage.cache_read_tokens, 20);
    assert!((usage.total_cost_usd - 0.005).abs() < f64::EPSILON);
    assert_eq!(usage.total_api_ms, 1200);
    assert_eq!(usage.request_count, 1);
}

#[test]
fn accumulate_multiple_deltas() {
    let mut usage = SessionUsage::default();
    let d1 = UsageDelta { input_tokens: 100, output_tokens: 50, ..Default::default() };
    let d2 = UsageDelta { input_tokens: 200, output_tokens: 75, ..Default::default() };
    usage.accumulate(&d1);
    usage.accumulate(&d2);
    assert_eq!(usage.input_tokens, 300);
    assert_eq!(usage.output_tokens, 125);
    assert_eq!(usage.request_count, 2);
}

#[test]
fn extract_from_result_entry() {
    let entry = json!({
        "type": "result",
        "costUSD": 0.012,
        "durationMs": 3400,
        "usage": {
            "input_tokens": 500,
            "output_tokens": 200,
            "cache_creation_input_tokens": 30,
            "cache_read_input_tokens": 100
        }
    });
    let delta = extract_usage_delta(&entry);
    assert!(delta.is_some());
    let d = delta.unwrap();
    assert_eq!(d.input_tokens, 500);
    assert_eq!(d.output_tokens, 200);
    assert_eq!(d.cache_creation_input_tokens, 30);
    assert_eq!(d.cache_read_input_tokens, 100);
    assert!((d.cost_usd - 0.012).abs() < f64::EPSILON);
    assert_eq!(d.duration_api_ms, 3400);
}

#[test]
fn extract_from_assistant_entry() {
    let entry = json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "usage": {
                "input_tokens": 3,
                "cache_creation_input_tokens": 7801,
                "cache_read_input_tokens": 17897,
                "output_tokens": 2,
                "service_tier": "standard"
            }
        },
        "requestId": "req_abc"
    });
    let d = extract_usage_delta(&entry).unwrap();
    assert_eq!(d.input_tokens, 3);
    assert_eq!(d.output_tokens, 2);
    assert_eq!(d.cache_creation_input_tokens, 7801);
    assert_eq!(d.cache_read_input_tokens, 17897);
}

#[test]
fn missing_usage_key_returns_none() {
    let entry = json!({ "type": "assistant", "message": {} });
    assert!(extract_usage_delta(&entry).is_none());
}

#[test]
fn zero_tokens_returns_none() {
    let entry = json!({
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0
        }
    });
    assert!(extract_usage_delta(&entry).is_none());
}

#[test]
fn missing_optional_fields_default_to_zero() {
    let entry = json!({
        "usage": {
            "input_tokens": 50,
            "output_tokens": 25
        }
    });
    let d = extract_usage_delta(&entry).unwrap();
    assert_eq!(d.cache_creation_input_tokens, 0);
    assert_eq!(d.cache_read_input_tokens, 0);
    assert!((d.cost_usd - 0.0).abs() < f64::EPSILON);
    assert_eq!(d.duration_api_ms, 0);
}

#[tokio::test]
async fn usage_state_accumulate_and_snapshot() -> anyhow::Result<()> {
    let state = UsageState::new();
    let mut rx = state.usage_tx.subscribe();

    state
        .accumulate(UsageDelta {
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.01,
            ..Default::default()
        })
        .await;

    let event = rx.recv().await?;
    assert_eq!(event.seq, 1);
    assert_eq!(event.cumulative.input_tokens, 100);
    assert_eq!(event.cumulative.request_count, 1);

    let snap = state.snapshot().await;
    assert_eq!(snap.input_tokens, 100);
    assert_eq!(snap.output_tokens, 50);
    Ok(())
}
