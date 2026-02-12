// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! OAuth token refresh with retries.

use std::time::Duration;

use crate::credential::oauth::TokenResponse;

/// Perform a single token refresh request.
pub async fn do_refresh(
    client: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    refresh_token: &str,
) -> anyhow::Result<TokenResponse> {
    let resp = client
        .post(token_url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("refresh failed ({status}): {text}");
    }

    let token: TokenResponse = resp.json().await?;
    Ok(token)
}

/// Refresh with exponential backoff retries.
pub async fn refresh_with_retries(
    client: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    refresh_token: &str,
    max_retries: u32,
) -> anyhow::Result<TokenResponse> {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    for attempt in 0..=max_retries {
        match do_refresh(client, token_url, client_id, refresh_token).await {
            Ok(token) => return Ok(token),
            Err(e) => {
                if attempt == max_retries {
                    return Err(e);
                }
                tracing::debug!(attempt, err = %e, "refresh attempt failed, retrying");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
            }
        }
    }

    anyhow::bail!("refresh exhausted all retries")
}
