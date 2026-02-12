// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Coop-mux: PTY multiplexing proxy for coop instances.

pub mod config;
pub mod error;
pub mod state;
pub mod transport;
pub mod upstream;

use std::sync::Arc;

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::MuxConfig;
use crate::state::MuxState;
use crate::transport::build_router;
use crate::upstream::health::spawn_health_checker;

/// Run the mux server until shutdown.
pub async fn run(config: MuxConfig) -> anyhow::Result<()> {
    let addr = format!("{}:{}", config.host, config.port);
    let shutdown = CancellationToken::new();

    let state = Arc::new(MuxState::new(config, shutdown.clone()));
    let router = build_router(Arc::clone(&state));

    let listener = TcpListener::bind(&addr).await?;
    tracing::info!("coop-mux listening on {addr}");

    spawn_health_checker(Arc::clone(&state));

    axum::serve(listener, router).with_graceful_shutdown(shutdown.cancelled_owned()).await?;

    Ok(())
}
