// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! RFC 8628 device code flow for re-authentication.

use std::time::Duration;

use crate::credential::oauth::{urlencoded, DeviceCodeResponse, TokenResponse};

/// Initiate the device authorization request.
pub async fn initiate_reauth(
    client: &reqwest::Client,
    device_auth_url: &str,
    client_id: &str,
) -> anyhow::Result<DeviceCodeResponse> {
    let body = urlencoded(&[("client_id", client_id)]);

    let resp = client
        .post(device_auth_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("device auth failed ({status}): {text}");
    }

    let device: DeviceCodeResponse = resp.json().await?;
    Ok(device)
}

/// Poll for the device code to be authorized, returning tokens when ready.
pub async fn poll_device_code(
    client: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    device_code: &str,
    interval_secs: u64,
    timeout_secs: u64,
) -> anyhow::Result<TokenResponse> {
    let interval = Duration::from_secs(interval_secs.max(1));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);

    loop {
        tokio::time::sleep(interval).await;

        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("device code polling timed out");
        }

        let body = urlencoded(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("client_id", client_id),
            ("device_code", device_code),
        ]);

        let resp = client
            .post(token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await?;

        if resp.status().is_success() {
            let token: TokenResponse = resp.json().await?;
            return Ok(token);
        }

        let text = resp.text().await.unwrap_or_default();

        // RFC 8628: "authorization_pending" means keep polling.
        if text.contains("authorization_pending") {
            continue;
        }
        // "slow_down" means increase interval (we just continue at same pace).
        if text.contains("slow_down") {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        // Other errors are fatal.
        anyhow::bail!("device code poll error: {text}");
    }
}
