// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Session loop: core runtime orchestrating PTY, screen, ring buffer,
//! detection, and transport layers.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::driver::{Detector, ExitStatus, OptionParser};
use crate::backend::{Backend, Boxed};
use crate::switch::SwitchRequest;
use crate::transport::Store;

mod groom;
pub mod run;
pub mod transition;

pub use run::Session;

/// Runtime objects for building a new [`Session`] (not derivable from [`Config`](crate::config::Config)).
pub struct SessionConfig {
    pub backend: Box<dyn Backend>,
    pub detectors: Vec<Box<dyn Detector>>,
    pub store: Arc<Store>,
    pub shutdown: CancellationToken,
    /// Driver-provided parser for extracting numbered option labels from
    /// rendered screen lines during prompt enrichment.
    pub option_parser: Option<OptionParser>,
}

impl SessionConfig {
    pub fn new(store: Arc<Store>, backend: impl Boxed) -> Self {
        Self {
            backend: backend.boxed(),
            store,
            detectors: Vec::new(),
            shutdown: CancellationToken::new(),
            option_parser: None,
        }
    }

    pub fn with_detectors(mut self, detectors: Vec<Box<dyn Detector>>) -> Self {
        self.detectors = detectors;
        self
    }

    pub fn with_shutdown(mut self, shutdown: CancellationToken) -> Self {
        self.shutdown = shutdown;
        self
    }

    pub fn with_option_parser(mut self, parser: OptionParser) -> Self {
        self.option_parser = Some(parser);
        self
    }
}

/// What happened when the session loop exited.
#[derive(Debug)]
pub enum SessionOutcome {
    /// The backend exited (process terminated).
    Exit(ExitStatus),
    /// A switch was requested and the backend has been drained.
    /// The caller should respawn with new credentials.
    Switch(SwitchRequest),
}

#[cfg(test)]
#[path = "../session_tests.rs"]
mod tests;
