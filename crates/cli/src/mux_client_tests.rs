// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;
use std::collections::HashMap;

fn env_from(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> =
        vars.iter().map(|&(k, v)| (k.to_owned(), v.to_owned())).collect();
    move |name: &str| map.get(name).cloned()
}

#[test]
fn detect_metadata_returns_agent_outside_k8s() {
    let result = detect_metadata_with("claude", &[], env_from(&[]));
    assert_eq!(result.get("agent"), Some(&Value::String("claude".into())));
    // No k8s key when not in Kubernetes.
    assert_eq!(result.get("k8s"), None);
}

#[test]
fn detect_metadata_returns_k8s_when_env_set() {
    let result = detect_metadata_with(
        "claude",
        &[],
        env_from(&[("KUBERNETES_SERVICE_HOST", "10.0.0.1"), ("POD_NAME", "my-pod-abc123")]),
    );

    assert_eq!(result.get("agent"), Some(&Value::String("claude".into())));
    let k8s = result.get("k8s").expect("expected k8s key");
    assert_eq!(k8s.get("pod"), Some(&Value::String("my-pod-abc123".into())));
    // No other fields should be present since we didn't set them.
    assert_eq!(k8s.get("namespace"), None);
    assert_eq!(k8s.get("node"), None);
    assert_eq!(k8s.get("ip"), None);
    assert_eq!(k8s.get("service_account"), None);
}

#[test]
fn detect_metadata_pod_name_takes_priority_over_hostname() {
    let result = detect_metadata_with(
        "claude",
        &[],
        env_from(&[
            ("KUBERNETES_SERVICE_HOST", "10.0.0.1"),
            ("POD_NAME", "real-pod"),
            ("HOSTNAME", "hostname-fallback"),
        ]),
    );

    let k8s = result.get("k8s").expect("expected k8s key");
    assert_eq!(k8s.get("pod"), Some(&Value::String("real-pod".into())));
}

#[test]
fn detect_metadata_falls_back_to_hostname() {
    let result = detect_metadata_with(
        "claude",
        &[],
        env_from(&[("KUBERNETES_SERVICE_HOST", "10.0.0.1"), ("HOSTNAME", "hostname-fallback")]),
    );

    let k8s = result.get("k8s").expect("expected k8s key");
    assert_eq!(k8s.get("pod"), Some(&Value::String("hostname-fallback".into())));
}

#[test]
fn detect_metadata_all_fields() {
    let result = detect_metadata_with(
        "gemini",
        &[],
        env_from(&[
            ("KUBERNETES_SERVICE_HOST", "10.0.0.1"),
            ("POD_NAME", "my-pod"),
            ("POD_NAMESPACE", "default"),
            ("NODE_NAME", "node-1"),
            ("POD_IP", "10.0.1.5"),
            ("POD_SERVICE_ACCOUNT", "my-sa"),
        ]),
    );

    assert_eq!(result.get("agent"), Some(&Value::String("gemini".into())));
    let k8s = result.get("k8s").expect("expected k8s key");
    assert_eq!(k8s.get("pod"), Some(&Value::String("my-pod".into())));
    assert_eq!(k8s.get("namespace"), Some(&Value::String("default".into())));
    assert_eq!(k8s.get("node"), Some(&Value::String("node-1".into())));
    assert_eq!(k8s.get("ip"), Some(&Value::String("10.0.1.5".into())));
    assert_eq!(k8s.get("service_account"), Some(&Value::String("my-sa".into())));
}

#[test]
fn detect_metadata_flat_labels() {
    let labels: Vec<String> = vec!["role=worker".into(), "team=infra".into()];
    let result = detect_metadata_with("claude", &labels, env_from(&[]));

    assert_eq!(result.get("agent"), Some(&Value::String("claude".into())));
    assert_eq!(result.get("role"), Some(&Value::String("worker".into())));
    assert_eq!(result.get("team"), Some(&Value::String("infra".into())));
}

#[test]
fn detect_metadata_dot_notation_nesting() {
    let labels: Vec<String> = vec!["gastown.role=worker".into()];
    let result = detect_metadata_with("claude", &labels, env_from(&[]));

    let gastown = result.get("gastown").expect("expected gastown key");
    assert_eq!(gastown.get("role"), Some(&Value::String("worker".into())));
}

#[test]
fn detect_metadata_deep_nesting() {
    let labels: Vec<String> = vec!["a.b.c=v".into()];
    let result = detect_metadata_with("claude", &labels, env_from(&[]));

    let a = result.get("a").expect("expected a key");
    let b = a.get("b").expect("expected b key");
    assert_eq!(b.get("c"), Some(&Value::String("v".into())));
}

#[test]
fn detect_metadata_labels_and_k8s_coexist() {
    let labels: Vec<String> = vec!["role=worker".into()];
    let result = detect_metadata_with(
        "claude",
        &labels,
        env_from(&[("KUBERNETES_SERVICE_HOST", "10.0.0.1"), ("POD_NAME", "my-pod")]),
    );

    assert_eq!(result.get("agent"), Some(&Value::String("claude".into())));
    assert_eq!(result.get("role"), Some(&Value::String("worker".into())));
    let k8s = result.get("k8s").expect("expected k8s key");
    assert_eq!(k8s.get("pod"), Some(&Value::String("my-pod".into())));
}

#[test]
fn detect_metadata_malformed_label_skipped() {
    let labels: Vec<String> = vec!["no-equals-sign".into(), "valid=yes".into()];
    let result = detect_metadata_with("claude", &labels, env_from(&[]));

    // Malformed label is skipped, valid one is present.
    assert_eq!(result.get("no-equals-sign"), None);
    assert_eq!(result.get("valid"), Some(&Value::String("yes".into())));
}

#[test]
fn detect_metadata_empty_value_is_valid() {
    let labels: Vec<String> = vec!["key=".into()];
    let result = detect_metadata_with("claude", &labels, env_from(&[]));

    assert_eq!(result.get("key"), Some(&Value::String(String::new())));
}
