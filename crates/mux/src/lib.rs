// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Coop-mux: PTY multiplexing proxy for coop instances.

pub mod config;
pub mod credential;
pub mod error;
pub mod state;
pub mod transport;
pub mod upstream;

use std::sync::Arc;

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::MuxConfig;
use crate::credential::broker::CredentialBroker;
use crate::state::MuxState;
use crate::transport::build_router;
use crate::upstream::health::spawn_health_checker;

/// Run the mux server until shutdown.
pub async fn run(config: MuxConfig) -> anyhow::Result<()> {
    let addr = format!("{}:{}", config.host, config.port);
    let shutdown = CancellationToken::new();

    let mut state = MuxState::new(config.clone(), shutdown.clone());

    // Optionally initialize credential broker.
    if let Some(ref cred_path) = config.credential_config {
        let contents = std::fs::read_to_string(cred_path)?;
        let cred_config: crate::credential::CredentialConfig = serde_json::from_str(&contents)?;
        let (event_tx, event_rx) = crate::credential::credential_channel();
        let broker = CredentialBroker::new(cred_config.clone(), event_tx);

        // Load persisted credentials if available.
        if let Some(ref persist_path) = cred_config.persist_path {
            if persist_path.exists() {
                match crate::credential::persist::load(persist_path) {
                    Ok(persisted) => broker.load_persisted(&persisted).await,
                    Err(e) => tracing::warn!(err = %e, "failed to load persisted credentials"),
                }
            }
        }

        state.credential_broker = Some(Arc::clone(&broker));
        let state_arc = Arc::new(state);

        // Spawn refresh loops and distributor.
        broker.spawn_refresh_loops();
        crate::credential::distributor::spawn_distributor(Arc::clone(&state_arc), event_rx);

        let router = build_router(Arc::clone(&state_arc));
        let listener = TcpListener::bind(&addr).await?;
        tracing::info!("coop-mux listening on {addr} (credentials enabled)");
        spawn_health_checker(Arc::clone(&state_arc));
        axum::serve(listener, router).with_graceful_shutdown(shutdown.cancelled_owned()).await?;
    } else {
        let state = Arc::new(state);
        let router = build_router(Arc::clone(&state));
        let listener = TcpListener::bind(&addr).await?;
        tracing::info!("coop-mux listening on {addr}");
        spawn_health_checker(Arc::clone(&state));
        axum::serve(listener, router).with_graceful_shutdown(shutdown.cancelled_owned()).await?;
    }

    Ok(())
}
