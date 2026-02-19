// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Coopmux: PTY multiplexing proxy for coop instances.

pub mod config;
pub mod credential;
pub mod error;
pub mod state;
pub mod transport;
pub mod upstream;

use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::config::MuxConfig;
use crate::credential::broker::CredentialBroker;
use crate::credential::CredentialConfig;
use crate::state::MuxState;
#[cfg(not(debug_assertions))]
use crate::transport::build_router;
#[cfg(debug_assertions)]
use crate::transport::build_router_hot;
use crate::upstream::health::spawn_health_checker;
use crate::upstream::prewarm::spawn_prewarm_task;

/// Optional NATS event publishing configuration.
///
/// Passed separately from [`MuxConfig`] because these args live on the
/// binary's CLI struct rather than the library config.
pub struct NatsConfig {
    pub url: String,
    pub token: Option<String>,
    pub prefix: String,
}

/// Run the mux server until shutdown.
pub async fn run(config: MuxConfig, nats: Option<NatsConfig>) -> anyhow::Result<()> {
    let addr = format!("{}:{}", config.host, config.port);
    let shutdown = CancellationToken::new();

    let mut state = MuxState::new(config.clone(), shutdown.clone());

    // Always initialize credential broker (empty config if no file provided).
    let cred_config = match config.credential_config {
        Some(ref cred_path) => {
            let contents = std::fs::read_to_string(cred_path)?;
            serde_json::from_str::<CredentialConfig>(&contents)?
        }
        None => CredentialConfig { accounts: vec![] },
    };

    let (event_tx, event_rx) = broadcast::channel(64);
    let cred_bridge_rx = event_tx.subscribe();
    let broker = CredentialBroker::new(cred_config, event_tx);

    state.credential_broker = Some(Arc::clone(&broker));

    // Spawn distributor (pushes credentials to sessions on events).
    let state = Arc::new(state);
    crate::credential::distributor::spawn_distributor(Arc::clone(&state), event_rx);

    // NATS credential event publishing removed — static API keys don't need
    // periodic refresh notifications. The nats arg is kept for API compat but
    // ignored for credential events.
    let _ = nats;

    // Bridge credential events into the MuxEvent broadcast channel.
    {
        let mux_event_tx = state.feed.event_tx.clone();
        tokio::spawn(async move {
            let mut rx = cred_bridge_rx;
            loop {
                match rx.recv().await {
                    Ok(e) => {
                        let _ = mux_event_tx.send(crate::state::MuxEvent::from_credential(&e));
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
    }

    let has_creds = config.credential_config.is_some();
    if has_creds {
        tracing::info!("coopmux listening on {addr} (credentials enabled)");
    } else {
        tracing::info!("coopmux listening on {addr}");
    }
    spawn_health_checker(Arc::clone(&state));
    spawn_prewarm_task(
        Arc::clone(&state),
        Arc::clone(&state.prewarm),
        config.prewarm_poll_interval(),
        shutdown.clone(),
    );
    #[cfg(debug_assertions)]
    let router = build_router_hot(state, config.hot);
    #[cfg(not(debug_assertions))]
    let router = build_router(state);
    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, router).with_graceful_shutdown(shutdown.cancelled_owned()).await?;

    Ok(())
}
