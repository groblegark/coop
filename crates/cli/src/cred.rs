// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! CLI credential management — `coop cred` subcommands.

use std::io::Write;

use clap::{Parser, Subcommand};
use serde::Deserialize;

#[derive(Parser)]
pub struct CredArgs {
    /// Coop server URL (e.g. http://localhost:8080)
    #[arg(long, env = "COOP_URL", default_value = "http://localhost:8080")]
    pub url: String,

    /// Auth token for the coop API
    #[arg(long, env = "COOP_AUTH_TOKEN")]
    pub token: Option<String>,

    #[command(subcommand)]
    pub command: CredCommand,
}

#[derive(Subcommand)]
pub enum CredCommand {
    /// List all credential accounts and their status
    List,
    /// Show detailed status for all accounts
    Status,
    /// Trigger re-authentication for a revoked account (runs on broker)
    Reauth {
        /// Account name (omit to re-auth first revoked account)
        account: Option<String>,
    },
    /// Add a new credential via local OAuth device code flow
    Add {
        /// Account name (e.g. "personal", "work", "team-max")
        name: String,
        /// OAuth provider
        #[arg(long, default_value = "claude")]
        provider: String,
        /// OAuth token endpoint URL
        #[arg(long, default_value = "https://platform.claude.com/v1/oauth/token")]
        token_url: String,
        /// OAuth device authorization URL
        #[arg(long, default_value = "https://console.anthropic.com/v1/oauth/device/code")]
        device_url: String,
        /// OAuth client ID
        #[arg(long, default_value = "9d1c250a-e61b-44d9-88ed-5944d1962f5e")]
        client_id: String,
        /// Don't open the browser automatically
        #[arg(long)]
        no_browser: bool,
    },
}

#[derive(Deserialize)]
struct StatusResponse {
    accounts: Vec<Account>,
}

#[derive(Deserialize)]
struct Account {
    name: String,
    provider: String,
    status: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    expires_in_secs: Option<u64>,
}

#[derive(Deserialize)]
struct ReauthResponse {
    account: String,
    auth_url: String,
    user_code: String,
}

/// RFC 8628 device authorization response.
#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default = "default_expires")]
    expires_in: u64,
    #[serde(default = "default_interval")]
    interval: u64,
}

fn default_expires() -> u64 {
    900
}
fn default_interval() -> u64 {
    5
}

/// Token response from the OAuth token endpoint.
#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

fn build_client(token: &Option<String>) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(t) = token {
        if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {t}")) {
            headers.insert(reqwest::header::AUTHORIZATION, val);
        }
    }
    reqwest::Client::builder().default_headers(headers).build().unwrap_or_default()
}

fn format_expires(secs: Option<u64>) -> String {
    match secs {
        Some(s) => {
            let m = s / 60;
            let rem = s % 60;
            format!("{m}m {rem:02}s")
        }
        None => "\u{2014}".to_string(),
    }
}

async fn fetch_status(client: &reqwest::Client, url: &str) -> Result<StatusResponse, String> {
    let resp = client
        .get(format!("{url}/api/v1/credentials/status"))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("server returned {}", resp.status()));
    }

    resp.json::<StatusResponse>().await.map_err(|e| format!("invalid response: {e}"))
}

fn print_table(accounts: &[Account], detailed: bool) {
    // Column widths
    let name_w = accounts.iter().map(|a| a.name.len()).max().unwrap_or(0).max(7);
    let prov_w = accounts.iter().map(|a| a.provider.len()).max().unwrap_or(0).max(8);

    println!("{:<name_w$}  {:<prov_w$}  {:<10}  {}", "ACCOUNT", "PROVIDER", "STATUS", "EXPIRES IN");

    for a in accounts {
        let expires = if a.status == "healthy" {
            format_expires(a.expires_in_secs)
        } else {
            "\u{2014}".to_string()
        };
        println!("{:<name_w$}  {:<prov_w$}  {:<10}  {}", a.name, a.provider, a.status, expires);

        if detailed {
            if let Some(err) = &a.error {
                println!("  error: {err}");
            }
        }
    }
}

pub async fn run(args: CredArgs) -> i32 {
    let client = build_client(&args.token);
    let url = args.url.trim_end_matches('/');

    match args.command {
        CredCommand::List => match fetch_status(&client, url).await {
            Ok(resp) => {
                print_table(&resp.accounts, false);
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },

        CredCommand::Status => match fetch_status(&client, url).await {
            Ok(resp) => {
                print_table(&resp.accounts, true);
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },

        CredCommand::Reauth { account } => {
            let body = match &account {
                Some(name) => serde_json::json!({ "account": name }),
                None => serde_json::json!({}),
            };

            let resp = match client
                .post(format!("{url}/api/v1/credentials/reauth"))
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("error: request failed: {e}");
                    return 1;
                }
            };

            if !resp.status().is_success() {
                eprintln!("error: server returned {}", resp.status());
                return 1;
            }

            let reauth: ReauthResponse = match resp.json().await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("error: invalid response: {e}");
                    return 1;
                }
            };

            println!("Initiating re-authentication for \"{}\"...", reauth.account);
            println!();
            println!("Open this URL to authenticate:");
            println!("  {}", reauth.auth_url);
            println!();
            println!("User code: {}", reauth.user_code);
            println!();
            println!("Waiting for authentication... (press Ctrl+C to cancel)");

            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;

                match fetch_status(&client, url).await {
                    Ok(status) => {
                        if let Some(acct) =
                            status.accounts.iter().find(|a| a.name == reauth.account)
                        {
                            if acct.status == "healthy" {
                                println!(
                                    "\u{2713} Authentication successful! Account \"{}\" is now healthy.",
                                    reauth.account
                                );
                                return 0;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("warning: poll failed: {e}");
                    }
                }
            }
        }

        CredCommand::Add { name, provider, token_url, device_url, client_id, no_browser } => {
            run_add(&client, url, &name, &provider, &token_url, &device_url, &client_id, no_browser)
                .await
        }
    }
}

/// Run the local device code flow and seed credentials to the broker.
async fn run_add(
    client: &reqwest::Client,
    broker_url: &str,
    name: &str,
    provider: &str,
    token_url: &str,
    device_url: &str,
    client_id: &str,
    no_browser: bool,
) -> i32 {
    println!("Adding credential account \"{name}\"...\n");

    // Step 1: Request device code.
    let device = match request_device_code(device_url, client_id).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: device code request failed: {e}");
            return 1;
        }
    };

    let auth_url = device.verification_uri_complete.as_deref().unwrap_or(&device.verification_uri);

    println!("Open this URL in your browser to authorize:\n");
    println!("  {auth_url}\n");
    if !device.user_code.is_empty() {
        println!("Enter code: {}\n", device.user_code);
    }

    // Open browser unless --no-browser.
    if !no_browser {
        open_browser(auth_url);
    }

    print!("Waiting for authorization...");
    let _ = std::io::stdout().flush();

    // Step 2: Poll for token.
    let token = match poll_for_token(token_url, client_id, &device).await {
        Ok(t) => t,
        Err(e) => {
            println!();
            eprintln!("error: {e}");
            return 1;
        }
    };

    println!(" \u{2713}");

    let access_token = match token.access_token {
        Some(ref t) => t.clone(),
        None => {
            eprintln!("error: no access token in response");
            return 1;
        }
    };

    // Step 3: Seed credentials to the broker.
    let seed_body = serde_json::json!({
        "account": name,
        "access_token": access_token,
        "refresh_token": token.refresh_token.as_deref().unwrap_or(""),
        "expires_in": token.expires_in.unwrap_or(3600),
    });

    let resp = match client
        .post(format!("{broker_url}/api/v1/credentials/seed"))
        .json(&seed_body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: seed request failed: {e}");
            return 1;
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        eprintln!("error: broker rejected seed (HTTP {status}): {body}");
        eprintln!(
            "\nHint: the account \"{name}\" must be configured in the broker's agent-config."
        );
        eprintln!("Add it to the credentials.accounts array in the broker ConfigMap.");
        return 1;
    }

    let expires_min = token.expires_in.unwrap_or(3600) / 60;
    println!(
        "\n\u{2713} Account \"{name}\" ({provider}) seeded successfully (expires in {expires_min}m)"
    );

    // Show updated status.
    if let Ok(status) = fetch_status(client, broker_url).await {
        println!();
        print_table(&status.accounts, false);
    }

    0
}

/// Request a device code from the OAuth authorization server.
async fn request_device_code(
    device_url: &str,
    client_id: &str,
) -> Result<DeviceCodeResponse, String> {
    let resp = reqwest::Client::new()
        .post(device_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!("client_id={}", urlencoded(client_id)))
        .send()
        .await
        .map_err(|e| format!("{e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {}", truncate(&body, 200)));
    }

    resp.json::<DeviceCodeResponse>().await.map_err(|e| format!("parse error: {e}"))
}

/// Poll the token endpoint until the user authorizes or the flow expires.
async fn poll_for_token(
    token_url: &str,
    client_id: &str,
    device: &DeviceCodeResponse,
) -> Result<TokenResponse, String> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(device.expires_in);
    let mut interval = std::time::Duration::from_secs(device.interval);
    let http = reqwest::Client::new();

    loop {
        tokio::time::sleep(interval).await;

        if tokio::time::Instant::now() > deadline {
            return Err("device code expired".to_owned());
        }

        let body = format!(
            "grant_type=urn:ietf:params:oauth:grant-type:device_code&client_id={}&device_code={}",
            urlencoded(client_id),
            urlencoded(&device.device_code),
        );

        let resp = match http
            .post(token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => continue,
        };

        let token: TokenResponse = match resp.json().await {
            Ok(t) => t,
            Err(_) => continue,
        };

        if let Some(ref err) = token.error {
            match err.as_str() {
                "authorization_pending" => {
                    print!(".");
                    let _ = std::io::stdout().flush();
                    continue;
                }
                "slow_down" => {
                    interval += std::time::Duration::from_secs(5);
                    continue;
                }
                "expired_token" => return Err("device code expired".to_owned()),
                "access_denied" => return Err("authorization denied by user".to_owned()),
                other => {
                    let desc = token.error_description.as_deref().unwrap_or("");
                    return Err(format!("{other}: {desc}"));
                }
            }
        }

        if token.access_token.is_some() {
            return Ok(token);
        }
    }
}

/// URL-encode a string for form values.
fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// Truncate a string for error display.
fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}

/// Open a URL in the default browser (best-effort, non-blocking).
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd").args(["/c", "start", url]).spawn();
    }
}
