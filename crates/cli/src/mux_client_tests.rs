// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;
use std::collections::HashMap;

fn env_from(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> =
        vars.iter().map(|&(k, v)| (k.to_owned(), v.to_owned())).collect();
    move |name: &str| map.get(name).cloned()
}

fn no_labels() -> impl Iterator<Item = (String, String)> {
    std::iter::empty()
}

#[test]
fn detect_metadata_returns_agent_outside_k8s() {
    let result = detect_metadata_with("claude", env_from(&[]), no_labels());
    assert_eq!(result.get("agent"), Some(&Value::String("claude".into())));
    // No k8s key when not in Kubernetes.
    assert_eq!(result.get("k8s"), None);
}

#[test]
fn detect_metadata_returns_k8s_when_env_set() {
    let result = detect_metadata_with(
        "claude",
        env_from(&[("KUBERNETES_SERVICE_HOST", "10.0.0.1"), ("POD_NAME", "my-pod-abc123")]),
        no_labels(),
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
        env_from(&[
            ("KUBERNETES_SERVICE_HOST", "10.0.0.1"),
            ("POD_NAME", "real-pod"),
            ("HOSTNAME", "hostname-fallback"),
        ]),
        no_labels(),
    );

    let k8s = result.get("k8s").expect("expected k8s key");
    assert_eq!(k8s.get("pod"), Some(&Value::String("real-pod".into())));
}

#[test]
fn detect_metadata_falls_back_to_hostname() {
    let result = detect_metadata_with(
        "claude",
        env_from(&[("KUBERNETES_SERVICE_HOST", "10.0.0.1"), ("HOSTNAME", "hostname-fallback")]),
        no_labels(),
    );

    let k8s = result.get("k8s").expect("expected k8s key");
    assert_eq!(k8s.get("pod"), Some(&Value::String("hostname-fallback".into())));
}

#[test]
fn detect_metadata_all_fields() {
    let result = detect_metadata_with(
        "gemini",
        env_from(&[
            ("KUBERNETES_SERVICE_HOST", "10.0.0.1"),
            ("POD_NAME", "my-pod"),
            ("POD_NAMESPACE", "default"),
            ("NODE_NAME", "node-1"),
            ("POD_IP", "10.0.1.5"),
            ("POD_SERVICE_ACCOUNT", "my-sa"),
        ]),
        no_labels(),
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
fn detect_metadata_includes_coop_labels() {
    let labels = vec![
        ("COOP_LABEL_ROLE".to_owned(), "worker".to_owned()),
        ("COOP_LABEL_TEAM".to_owned(), "infra".to_owned()),
        ("UNRELATED_VAR".to_owned(), "ignored".to_owned()),
    ];
    let result = detect_metadata_with("claude", env_from(&[]), labels.into_iter());

    assert_eq!(result.get("agent"), Some(&Value::String("claude".into())));
    assert_eq!(result.get("role"), Some(&Value::String("worker".into())));
    assert_eq!(result.get("team"), Some(&Value::String("infra".into())));
    assert_eq!(result.get("unrelated_var"), None);
}

#[test]
fn detect_metadata_labels_are_lowercased() {
    let labels = vec![("COOP_LABEL_MY_THING".to_owned(), "x".to_owned())];
    let result = detect_metadata_with("claude", env_from(&[]), labels.into_iter());

    assert_eq!(result.get("my_thing"), Some(&Value::String("x".into())));
    assert_eq!(result.get("MY_THING"), None);
}

#[test]
fn detect_metadata_labels_and_k8s_coexist() {
    let labels = vec![("COOP_LABEL_ROLE".to_owned(), "worker".to_owned())];
    let result = detect_metadata_with(
        "claude",
        env_from(&[("KUBERNETES_SERVICE_HOST", "10.0.0.1"), ("POD_NAME", "my-pod")]),
        labels.into_iter(),
    );

    assert_eq!(result.get("agent"), Some(&Value::String("claude".into())));
    assert_eq!(result.get("role"), Some(&Value::String("worker".into())));
    let k8s = result.get("k8s").expect("expected k8s key");
    assert_eq!(k8s.get("pod"), Some(&Value::String("my-pod".into())));
}

#[test]
fn detect_metadata_empty_label_suffix_ignored() {
    let labels = vec![("COOP_LABEL_".to_owned(), "empty".to_owned())];
    let result = detect_metadata_with("claude", env_from(&[]), labels.into_iter());

    // Only "agent" key should be present — empty suffix is skipped.
    assert_eq!(result.as_object().map(|m| m.len()), Some(1));
    assert_eq!(result.get("agent"), Some(&Value::String("claude".into())));
}
