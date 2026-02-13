// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! `coop cred` — manage mux credentials from the CLI.
//!
//! Talks to the mux server's credential endpoints via `COOP_MUX_URL`.

/// CLI arguments for `coop cred`.
#[derive(Debug, clap::Args)]
pub struct CredArgs {
    #[command(subcommand)]
    pub command: CredCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum CredCommand {
    /// List all credential accounts and their status.
    List,
    /// Seed initial tokens for an account.
    Seed(SeedArgs),
    /// Trigger device code re-authentication for an account.
    Reauth(ReauthArgs),
}

#[derive(Debug, clap::Args)]
pub struct SeedArgs {
    /// Account name.
    pub account: String,
    /// Access token.
    #[arg(long)]
    pub token: String,
    /// Refresh token (optional).
    #[arg(long)]
    pub refresh_token: Option<String>,
    /// Token TTL in seconds (optional).
    #[arg(long)]
    pub expires_in: Option<u64>,
}

#[derive(Debug, clap::Args)]
pub struct ReauthArgs {
    /// Account name (defaults to first account).
    pub account: Option<String>,
}

/// Run the `coop cred` subcommand. Returns a process exit code.
pub fn run(args: &CredArgs) -> i32 {
    let mux_url = match std::env::var("COOP_MUX_URL") {
        Ok(u) => u.trim_end_matches('/').to_owned(),
        Err(_) => {
            eprintln!("error: COOP_MUX_URL is not set");
            return 2;
        }
    };

    let mux_token = std::env::var("COOP_MUX_TOKEN").ok();
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    match &args.command {
        CredCommand::List => cmd_list(&client, &mux_url, mux_token.as_deref()),
        CredCommand::Seed(seed) => cmd_seed(&client, &mux_url, mux_token.as_deref(), seed),
        CredCommand::Reauth(reauth) => cmd_reauth(&client, &mux_url, mux_token.as_deref(), reauth),
    }
}

fn apply_auth(
    req: reqwest::blocking::RequestBuilder,
    token: Option<&str>,
) -> reqwest::blocking::RequestBuilder {
    match token {
        Some(t) => req.bearer_auth(t),
        None => req,
    }
}

fn cmd_list(client: &reqwest::blocking::Client, mux_url: &str, token: Option<&str>) -> i32 {
    let url = format!("{mux_url}/api/v1/credentials/status");
    let resp = match apply_auth(client.get(&url), token).send() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let status = resp.status();
    let text = resp.text().unwrap_or_default();

    if status.is_success() {
        // Pretty-print the JSON table.
        match serde_json::from_str::<Vec<serde_json::Value>>(&text) {
            Ok(accounts) => {
                if accounts.is_empty() {
                    println!("No credential accounts configured.");
                } else {
                    println!("{:<20} {:<12} {:<10}", "ACCOUNT", "STATUS", "PROVIDER");
                    println!("{}", "-".repeat(42));
                    for acct in &accounts {
                        let name = acct.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                        let status =
                            acct.get("status").and_then(|v| v.as_str()).unwrap_or("unknown");
                        let provider = acct.get("provider").and_then(|v| v.as_str()).unwrap_or("?");
                        println!("{name:<20} {status:<12} {provider:<10}");
                    }
                }
                0
            }
            Err(_) => {
                // Fallback: print raw JSON.
                println!("{text}");
                0
            }
        }
    } else {
        eprintln!("error ({status}): {text}");
        1
    }
}

fn cmd_seed(
    client: &reqwest::blocking::Client,
    mux_url: &str,
    token: Option<&str>,
    args: &SeedArgs,
) -> i32 {
    let url = format!("{mux_url}/api/v1/credentials/seed");
    let body = serde_json::json!({
        "account": args.account,
        "token": args.token,
        "refresh_token": args.refresh_token,
        "expires_in": args.expires_in,
    });

    let resp = match apply_auth(client.post(&url), token).json(&body).send() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let status = resp.status();
    let text = resp.text().unwrap_or_default();

    if status.is_success() {
        println!("Seeded account '{}'.", args.account);
        0
    } else {
        eprintln!("error ({status}): {text}");
        1
    }
}

fn cmd_reauth(
    client: &reqwest::blocking::Client,
    mux_url: &str,
    token: Option<&str>,
    args: &ReauthArgs,
) -> i32 {
    let url = format!("{mux_url}/api/v1/credentials/reauth");
    let body = serde_json::json!({
        "account": args.account,
    });

    let resp = match apply_auth(client.post(&url), token).json(&body).send() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let status = resp.status();
    let text = resp.text().unwrap_or_default();

    if status.is_success() {
        // Try to extract device code info.
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(auth_url) = val.get("auth_url").and_then(|v| v.as_str()) {
                let user_code = val.get("user_code").and_then(|v| v.as_str()).unwrap_or("?");
                println!("Visit: {auth_url}");
                println!("Code:  {user_code}");
                return 0;
            }
        }
        println!("{text}");
        0
    } else {
        eprintln!("error ({status}): {text}");
        1
    }
}
