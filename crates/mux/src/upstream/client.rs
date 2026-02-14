// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! HTTP client for communicating with a single upstream coop instance.

use reqwest::Client;

/// HTTP client wrapper for one upstream coop instance.
pub struct UpstreamClient {
    base_url: String,
    auth_token: Option<String>,
    client: Client,
}

impl UpstreamClient {
    pub fn new(base_url: String, auth_token: Option<String>) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self { base_url, auth_token, client }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth_token {
            Some(token) => req.bearer_auth(token),
            None => req,
        }
    }

    /// Check upstream health.
    pub async fn health(&self) -> anyhow::Result<serde_json::Value> {
        let resp = self.client.get(self.url("/api/v1/health")).send().await?;
        let value = resp.error_for_status()?.json().await?;
        Ok(value)
    }

    /// Fetch screen snapshot from upstream.
    pub async fn get_screen(&self) -> anyhow::Result<serde_json::Value> {
        let req = self.client.get(self.url("/api/v1/screen"));
        let resp = self.apply_auth(req).send().await?;
        let value = resp.error_for_status()?.json().await?;
        Ok(value)
    }

    /// Fetch status from upstream.
    pub async fn get_status(&self) -> anyhow::Result<serde_json::Value> {
        let req = self.client.get(self.url("/api/v1/status"));
        let resp = self.apply_auth(req).send().await?;
        let value = resp.error_for_status()?.json().await?;
        Ok(value)
    }

    /// Fetch agent state from upstream.
    pub async fn get_agent(&self) -> anyhow::Result<serde_json::Value> {
        let req = self.client.get(self.url("/api/v1/agent"));
        let resp = self.apply_auth(req).send().await?;
        let value = resp.error_for_status()?.json().await?;
        Ok(value)
    }

    /// POST JSON to an upstream endpoint and return the response body.
    pub async fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let req = self.client.post(self.url(path)).json(body);
        let resp = self.apply_auth(req).send().await?.error_for_status()?;
        let bytes = resp.bytes().await?;
        if bytes.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
}
