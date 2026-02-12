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
fn detect_metadata_returns_null_outside_k8s() {
    // No KUBERNETES_SERVICE_HOST in the lookup.
    let result = detect_metadata_with(env_from(&[]));
    assert_eq!(result, Value::Null);
}

#[test]
fn detect_metadata_returns_k8s_when_env_set() {
    let result = detect_metadata_with(env_from(&[
        ("KUBERNETES_SERVICE_HOST", "10.0.0.1"),
        ("POD_NAME", "my-pod-abc123"),
    ]));

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
    let result = detect_metadata_with(env_from(&[
        ("KUBERNETES_SERVICE_HOST", "10.0.0.1"),
        ("POD_NAME", "real-pod"),
        ("HOSTNAME", "hostname-fallback"),
    ]));

    let k8s = result.get("k8s").expect("expected k8s key");
    assert_eq!(k8s.get("pod"), Some(&Value::String("real-pod".into())));
}

#[test]
fn detect_metadata_falls_back_to_hostname() {
    let result = detect_metadata_with(env_from(&[
        ("KUBERNETES_SERVICE_HOST", "10.0.0.1"),
        ("HOSTNAME", "hostname-fallback"),
    ]));

    let k8s = result.get("k8s").expect("expected k8s key");
    assert_eq!(k8s.get("pod"), Some(&Value::String("hostname-fallback".into())));
}

#[test]
fn detect_metadata_all_fields() {
    let result = detect_metadata_with(env_from(&[
        ("KUBERNETES_SERVICE_HOST", "10.0.0.1"),
        ("POD_NAME", "my-pod"),
        ("POD_NAMESPACE", "default"),
        ("NODE_NAME", "node-1"),
        ("POD_IP", "10.0.1.5"),
        ("POD_SERVICE_ACCOUNT", "my-sa"),
    ]));

    let k8s = result.get("k8s").expect("expected k8s key");
    assert_eq!(k8s.get("pod"), Some(&Value::String("my-pod".into())));
    assert_eq!(k8s.get("namespace"), Some(&Value::String("default".into())));
    assert_eq!(k8s.get("node"), Some(&Value::String("node-1".into())));
    assert_eq!(k8s.get("ip"), Some(&Value::String("10.0.1.5".into())));
    assert_eq!(k8s.get("service_account"), Some(&Value::String("my-sa".into())));
}
