// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! OAuth 2.0 Device Authorization Grant (RFC 8628) helpers.

use std::time::Duration;

use crate::credential::oauth::{DeviceCodeResponse, TokenResponse};

/// Initiate device authorization by POSTing to the device auth endpoint.
pub async fn initiate_device_auth(
    client: &reqwest::Client,
    device_auth_url: &str,
    client_id: &str,
    scope: &str,
) -> anyhow::Result<DeviceCodeResponse> {
    let resp = client
        .post(device_auth_url)
        .form(&[("client_id", client_id), ("scope", scope)])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("device authorization failed ({status}): {text}");
    }

    let device: DeviceCodeResponse = resp.json().await?;
    Ok(device)
}

/// Poll the token endpoint until the user completes authorization or the code expires.
pub async fn poll_device_code(
    client: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    device_code: &str,
    interval: u64,
    expires_in: u64,
) -> anyhow::Result<TokenResponse> {
    let mut poll_interval = Duration::from_secs(interval.max(1));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(expires_in);

    loop {
        tokio::time::sleep(poll_interval).await;

        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("device code expired before user completed authorization");
        }

        let resp = client
            .post(token_url)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", client_id),
                ("device_code", device_code),
            ])
            .send()
            .await?;

        if resp.status().is_success() {
            let token: TokenResponse = resp.json().await?;
            return Ok(token);
        }

        let text = resp.text().await.unwrap_or_default();

        if text.contains("authorization_pending") {
            continue;
        }
        if text.contains("slow_down") {
            poll_interval += Duration::from_secs(5);
            continue;
        }

        anyhow::bail!("device code token request failed: {text}");
    }
}
