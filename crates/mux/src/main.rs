// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use clap::Parser;
use tracing::error;

use coop_mux::config::MuxConfig;

#[tokio::main]
async fn main() {
    let config = MuxConfig::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    if let Err(e) = coop_mux::run(config).await {
        error!("fatal: {e:#}");
        std::process::exit(1);
    }
}
