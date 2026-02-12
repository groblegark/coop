// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! CLI credential management — `coop cred` subcommands.

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
    /// Trigger re-authentication for a revoked account
    Reauth {
        /// Account name (omit to re-auth first revoked account)
        account: Option<String>,
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

fn build_client(token: &Option<String>) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(t) = token {
        if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {t}")) {
            headers.insert(reqwest::header::AUTHORIZATION, val);
        }
    }
    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap_or_default()
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

    resp.json::<StatusResponse>()
        .await
        .map_err(|e| format!("invalid response: {e}"))
}

fn print_table(accounts: &[Account], detailed: bool) {
    // Column widths
    let name_w = accounts
        .iter()
        .map(|a| a.name.len())
        .max()
        .unwrap_or(0)
        .max(7);
    let prov_w = accounts
        .iter()
        .map(|a| a.provider.len())
        .max()
        .unwrap_or(0)
        .max(8);

    println!(
        "{:<name_w$}  {:<prov_w$}  {:<10}  {}",
        "ACCOUNT", "PROVIDER", "STATUS", "EXPIRES IN"
    );

    for a in accounts {
        let expires = if a.status == "healthy" {
            format_expires(a.expires_in_secs)
        } else {
            "\u{2014}".to_string()
        };
        println!(
            "{:<name_w$}  {:<prov_w$}  {:<10}  {}",
            a.name, a.provider, a.status, expires
        );

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
                        if let Some(acct) = status
                            .accounts
                            .iter()
                            .find(|a| a.name == reauth.account)
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
    }
}
