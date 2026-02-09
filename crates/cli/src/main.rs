// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use clap::Parser;
use tracing::error;

use coop::config::Config;

#[tokio::main]
async fn main() {
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
