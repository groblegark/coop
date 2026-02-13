// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

#[test]
fn code_verifier_is_valid_length() -> anyhow::Result<()> {
    let v = generate_code_verifier();
    assert!(v.len() >= 43 && v.len() <= 128, "verifier length {} out of range", v.len());
    Ok(())
}

#[test]
fn code_challenge_is_deterministic() -> anyhow::Result<()> {
    let verifier = "test-verifier-string";
    let c1 = compute_code_challenge(verifier);
    let c2 = compute_code_challenge(verifier);
    assert_eq!(c1, c2);
    assert!(!c1.is_empty());
    Ok(())
}

#[test]
fn state_is_unique() -> anyhow::Result<()> {
    let s1 = generate_state();
    let s2 = generate_state();
    assert_ne!(s1, s2);
    Ok(())
}

#[test]
fn build_auth_url_includes_params() -> anyhow::Result<()> {
    let url = build_auth_url(
        "https://example.com/authorize",
        "client-123",
        "http://localhost/callback",
        "openid",
        "challenge-abc",
        "state-xyz",
    );
    assert!(url.starts_with("https://example.com/authorize?code=true&"));
    assert!(url.contains("client_id=client-123"));
    assert!(url.contains("response_type=code"));
    assert!(url.contains("code_challenge=challenge-abc"));
    assert!(url.contains("code_challenge_method=S256"));
    assert!(url.contains("state=state-xyz"));
    Ok(())
}

#[test]
fn build_auth_url_matches_claude_code_param_order() -> anyhow::Result<()> {
    let url = build_auth_url(
        "https://claude.ai/oauth/authorize",
        "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
        "https://platform.claude.com/oauth/code/callback",
        "user:profile user:inference",
        "challenge-abc",
        "state-xyz",
    );
    // Parameter order: code=true, client_id, response_type, redirect_uri, scope, code_challenge, code_challenge_method, state
    let q = url.split('?').nth(1).unwrap();
    let keys: Vec<&str> = q.split('&').map(|p| p.split('=').next().unwrap()).collect();
    assert_eq!(
        keys,
        [
            "code",
            "client_id",
            "response_type",
            "redirect_uri",
            "scope",
            "code_challenge",
            "code_challenge_method",
            "state"
        ],
    );
    // Spaces in scope encoded as +
    assert!(url.contains("scope=user%3Aprofile+user%3Ainference"));
    Ok(())
}
