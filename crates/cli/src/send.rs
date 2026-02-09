// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! `coop send` — resolve a stop hook from inside the PTY.
//!
//! Posts a JSON body to `$COOP_URL/api/v1/hooks/stop/resolve` and prints the
//! response. This replaces the raw `curl` command that block-reason messages
//! previously suggested.

/// Run the `coop send` subcommand. Returns a process exit code.
///
/// `body_arg` is the optional first positional argument (JSON string).
/// If omitted, posts `{}`.
pub fn run(body_arg: Option<&str>) -> i32 {
    let coop_url = match std::env::var("COOP_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("error: COOP_URL is not set");
            return 2;
        }
    };

    send(&coop_url, body_arg)
}

/// Inner implementation: resolve the stop hook given a base URL and optional
/// JSON body argument.
fn send(coop_url: &str, body_arg: Option<&str>) -> i32 {
    let url = format!(
        "{}/api/v1/hooks/stop/resolve",
        coop_url.trim_end_matches('/')
    );
    let body_str = body_arg.unwrap_or("{}");

    let body: serde_json::Value = match serde_json::from_str(body_str) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: invalid JSON body: {e}");
            return 2;
        }
    };

    let client = reqwest::blocking::Client::new();
    let resp = match client.post(&url).json(&body).send() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: request failed: {e}");
            return 1;
        }
    };

    let status = resp.status();
    let text = resp.text().unwrap_or_default();

    if !text.is_empty() {
        println!("{text}");
    }

    if status.is_success() {
        0
    } else {
        1
    }
}

#[cfg(test)]
#[path = "send_tests.rs"]
mod tests;
