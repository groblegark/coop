// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! HTTP request/response types and axum handler implementations.

mod agent;
mod broker;
mod credential;
mod env;
mod events;
mod hooks;
mod mux;
mod record;
mod screen;
mod switch;
mod transcript;
mod usage;

pub use agent::*;
pub use broker::*;
pub use credential::*;
pub use env::*;
pub use events::*;
pub use hooks::*;
pub use mux::*;
pub use record::*;
pub use screen::*;
pub use switch::*;
pub use transcript::*;
pub use usage::*;

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;

use crate::transport::state::Store;

// -- Lifecycle ----------------------------------------------------------------

/// `POST /api/v1/shutdown` — initiate graceful coop shutdown.
pub async fn shutdown(State(s): State<Arc<Store>>) -> impl IntoResponse {
    s.lifecycle.shutdown.cancel();
    Json(serde_json::json!({ "accepted": true }))
}

#[cfg(test)]
mod screen_tests;

#[cfg(test)]
mod agent_tests;

#[cfg(test)]
mod hooks_tests;

#[cfg(test)]
mod transcript_tests;

#[cfg(test)]
mod profile_tests;

#[cfg(test)]
mod switch_tests;

#[cfg(test)]
mod usage_tests;

#[cfg(test)]
mod env_tests;
