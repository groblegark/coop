// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! OAuth authorization code + PKCE (RFC 7636) helpers.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::Rng;
use sha2::{Digest, Sha256};

use crate::credential::oauth::TokenResponse;

/// Generate a PKCE code verifier (43-128 char URL-safe random string).
pub fn generate_code_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Compute code_challenge = base64url_nopad(sha256(verifier)).
pub fn compute_code_challenge(verifier: &str) -> String {
    let hash = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}

/// Generate a random state parameter (32 bytes → 43 chars, matching Claude Code).
pub fn generate_state() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Build the full authorization URL with PKCE parameters.
///
/// Parameter order matches Claude Code CLI exactly.
pub fn build_auth_url(
    auth_url: &str,
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    code_challenge: &str,
    state: &str,
) -> String {
    format!(
        "{auth_url}?code=true\
         &client_id={client_id}\
         &response_type=code\
         &redirect_uri={redirect_uri}\
         &scope={scope}\
         &code_challenge={code_challenge}\
         &code_challenge_method=S256\
         &state={state}",
        client_id = urlencoding(client_id),
        redirect_uri = urlencoding(redirect_uri),
        scope = urlencoding(scope),
        code_challenge = urlencoding(code_challenge),
        state = urlencoding(state),
    )
}

/// Exchange an authorization code for tokens (JSON body, matching Claude Code).
pub async fn exchange_code(
    client: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> anyhow::Result<TokenResponse> {
    let json_body = serde_json::json!({
        "grant_type": "authorization_code",
        "client_id": client_id,
        "code": code,
        "redirect_uri": redirect_uri,
        "code_verifier": code_verifier,
    });

    let resp = client
        .post(token_url)
        .header("Content-Type", "application/json")
        .body(json_body.to_string())
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("token exchange failed ({status}): {text}");
    }

    let token: TokenResponse = resp.json().await?;
    Ok(token)
}

/// Form-style encoding for URL query parameters (spaces as `+`).
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(char::from(HEX[(b >> 4) as usize]));
                out.push(char::from(HEX[(b & 0xf) as usize]));
            }
        }
    }
    out
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

#[cfg(test)]
#[path = "pkce_tests.rs"]
mod tests;
