// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use clap::Parser;
use tracing::error;

use coop::config::Config;

#[tokio::main]
async fn main() {
    // Intercept `coop send` before clap parsing (Config uses trailing_var_arg).
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("send") {
        let body_arg = args.get(2).map(|s| s.as_str());
        std::process::exit(coop::send::run(body_arg));
    }

    let config = Config::parse();

    if let Err(e) = config.validate() {
        eprintln!("error: {e}");
        std::process::exit(2);
    }

    match coop::run::run(config).await {
        Ok(result) => {
            std::process::exit(result.status.code.unwrap_or(1));
        }
        Err(e) => {
            error!("fatal: {e:#}");
            std::process::exit(1);
        }
    }
}
