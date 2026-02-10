// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Top-level session runner — shared by `main` and integration tests.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::Instant;

use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use tracing_subscriber::EnvFilter;

use crate::config::{self, Config, GroomLevel};
use crate::driver::claude::resume;
use crate::driver::claude::setup as claude_setup;
use crate::driver::gemini::setup as gemini_setup;
use crate::driver::AgentType;
use crate::driver::{
    build_claude_driver, build_gemini_driver, AgentState, DetectorSinks, DriverContext,
    SessionSetup,
};
use crate::event::InputEvent;
use crate::pty::adapter::{AttachSpec, TmuxBackend};
use crate::pty::spawn::NativePty;
use crate::pty::Backend;
use crate::ring::RingBuffer;
use crate::screen::Screen;
use crate::session::{Session, SessionConfig, SessionOutcome};
use crate::start::StartState;
use crate::stop::StopState;
use crate::switch::{SwitchRequest, SwitchState};
use crate::transcript::TranscriptState;
use crate::transport::grpc::CoopGrpc;
use crate::transport::state::{
    DetectionInfo, DriverState, LifecycleState, SessionSettings, TerminalState, TransportChannels,
};
use crate::transport::{build_health_router, build_router, Store};

/// Result of a completed session.
pub struct RunResult {
    pub status: crate::driver::ExitStatus,
    pub store: Arc<Store>,
}

/// A fully-prepared session ready to run.
///
/// Returned by [`prepare`] so callers (e.g. integration tests) can access
/// [`Store`] — including broadcast channels and the shutdown token — before
/// the blocking session loop starts.
pub struct PreparedSession {
    pub store: Arc<Store>,
    /// `Option` because `Session::run` takes ownership; after each run we
    /// build a new Session for the next switch iteration.
    session: Option<Session>,
    config: Config,
    /// Receivers that survive across switch iterations. `Session::run()`
    /// borrows these via `&mut` rather than consuming them.
    consumer_input_rx: mpsc::Receiver<InputEvent>,
    switch_rx: mpsc::Receiver<SwitchRequest>,
}

impl PreparedSession {
    /// Run the session loop to completion, handling credential switches.
    ///
    /// After the agent process exits, coop waits for either a switch request
    /// (restart with new credentials) or a shutdown signal (SIGTERM/SIGINT/API).
    /// Transport connections survive across switches.
    pub async fn run(mut self) -> anyhow::Result<RunResult> {
        loop {
            let session =
                self.session.take().ok_or_else(|| anyhow::anyhow!("no session available"))?;
            let outcome =
                session.run(&self.config, &mut self.consumer_input_rx, &mut self.switch_rx).await?;

            let req = match outcome {
                SessionOutcome::Exit(status) => {
                    // Agent exited — wait for a switch or shutdown.
                    if self.store.lifecycle.shutdown.is_cancelled() {
                        return Ok(RunResult { status, store: self.store });
                    }
                    info!(
                        "agent exited (code={:?}, signal={:?}), awaiting switch or shutdown",
                        status.code, status.signal
                    );
                    let req = tokio::select! {
                        req = self.switch_rx.recv() => match req {
                            Some(req) => req,
                            None => return Ok(RunResult { status, store: self.store }),
                        },
                        _ = self.store.lifecycle.shutdown.cancelled() => {
                            return Ok(RunResult { status, store: self.store });
                        }
                    };
                    req
                }
                SessionOutcome::Switch(req) => req,
            };

            self.execute_switch(&req).await?;
        }
    }

    /// Execute a credential switch: reset store state, prepare a new agent
    /// setup, spawn a new backend, and build a new Session.
    async fn execute_switch(&mut self, request: &SwitchRequest) -> anyhow::Result<()> {
        let agent_enum = self.config.agent_enum()?;

        // 1. Reset Store state for the new session iteration.
        self.store.terminal.reset(self.config.cols, self.config.rows, self.config.ring_size).await;
        self.store.driver.reset().await;
        self.store.ready.store(false, std::sync::atomic::Ordering::Release);

        // 2. Derive conversation ID from the current session log path.
        let conversation_id = {
            let log_path = self.store.switch.session_log_path.read().await;
            log_path
                .as_ref()
                .and_then(|p| p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_owned()))
        };

        // 3. Build resume state and prepare agent setup.
        let working_dir = std::env::current_dir()?;
        let coop_url = format!("http://127.0.0.1:{}", self.config.port.unwrap_or(0));
        let pristine = self.config.groom_level()? == GroomLevel::Pristine;
        let base_settings = self.store.switch.base_settings.as_ref();
        let mcp_config = self.store.switch.mcp_config.as_ref();

        let setup = match agent_enum {
            AgentType::Claude => {
                let resume_state = resume::ResumeState {
                    last_state: AgentState::Starting,
                    log_offset: 0,
                    conversation_id: conversation_id.clone(),
                };
                let log_path = self.store.switch.session_log_path.read().await;
                if let Some(ref lp) = *log_path {
                    Some(claude_setup::prepare(
                        &working_dir,
                        &coop_url,
                        base_settings,
                        mcp_config,
                        pristine,
                        Some((&resume_state, lp)),
                    )?)
                } else {
                    Some(claude_setup::prepare(
                        &working_dir,
                        &coop_url,
                        base_settings,
                        mcp_config,
                        pristine,
                        None,
                    )?)
                }
            }
            AgentType::Gemini => {
                Some(gemini_setup::prepare(&coop_url, base_settings, mcp_config, pristine)?)
            }
            _ => None,
        };

        // 4. Build command with extra args.
        let mut command = self.config.command.clone();
        if let Some(ref s) = setup {
            command.extend(s.extra_args.clone());
        }

        // 5. Merge credential env vars from request.
        let mut env_vars: Vec<(String, String)> =
            setup.as_ref().map(|s| s.env_vars.clone()).unwrap_or_default();
        if let Some(ref creds) = request.credentials {
            for (k, v) in creds {
                if let Some(existing) = env_vars.iter_mut().find(|(ek, _)| ek == k) {
                    existing.1 = v.clone();
                } else {
                    env_vars.push((k.clone(), v.clone()));
                }
            }
        }

        // 6. Build driver (detectors only — encoders already on SessionSettings).
        let sinks = || {
            DetectorSinks::default()
                .with_last_message(Arc::clone(&self.store.driver.last_message))
                .with_hook_tx(self.store.channels.hook_tx.clone())
                .with_message_tx(self.store.channels.message_tx.clone())
        };
        let driver = match agent_enum {
            AgentType::Claude => build_claude_driver(&self.config, setup.as_ref(), 0, sinks())?,
            AgentType::Gemini => build_gemini_driver(
                &self.config,
                setup.as_ref(),
                self.store.terminal.child_pid_fn(),
                self.store.terminal.ring_total_written_fn(),
                sinks(),
            )?,
            _ => DriverContext {
                nudge_encoder: None,
                respond_encoder: None,
                detectors: vec![],
                option_parser: None,
            },
        };

        // Add Tier 5 screen detector for Claude.
        let mut detectors = driver.detectors;
        if agent_enum == AgentType::Claude {
            detectors.push(Box::new(crate::driver::claude::screen::ClaudeScreenDetector::new(
                &self.config,
                self.store.terminal.snapshot_fn(),
            )));
            detectors.sort_by_key(|d| d.tier());
        }

        // 7. Spawn new backend.
        if command.is_empty() {
            anyhow::bail!("no command specified for switch");
        }
        let backend = NativePty::spawn(&command, self.config.cols, self.config.rows, &env_vars)?
            .with_reap_interval(self.config.reap_poll());

        // 8. Build new session config and Session.
        let shutdown = self.store.lifecycle.shutdown.clone();
        let mut session_config = SessionConfig::new(Arc::clone(&self.store), backend)
            .with_detectors(detectors)
            .with_shutdown(shutdown);
        if let Some(parser) = driver.option_parser {
            session_config = session_config.with_option_parser(parser);
        }
        self.session = Some(Session::new(&self.config, session_config));

        // 9. Update session log path for the next switch.
        if let Some(ref s) = setup {
            let mut log_path = self.store.switch.session_log_path.write().await;
            *log_path = s.session_log_path.clone();
        }

        // 10. Broadcast Starting transition.
        let last_message = self.store.driver.last_message.read().await.clone();
        let _ = self.store.channels.state_tx.send(crate::event::TransitionEvent {
            prev: AgentState::Switching,
            next: AgentState::Starting,
            seq: 0,
            cause: "switch".to_owned(),
            last_message,
        });

        info!("session switched");
        Ok(())
    }
}

/// Run a coop session to completion.
///
/// This is the full production codepath: prepare claude session, build driver,
/// spawn backend, start servers, and run the session loop.
pub async fn run(config: Config) -> anyhow::Result<RunResult> {
    prepare(config).await?.run().await
}

/// Initialize tracing/logging from config.
///
/// Uses `try_init` so it's safe to call multiple times (e.g. from tests).
pub fn init_tracing(config: &Config) {
    use tracing_subscriber::fmt;

    let filter = EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info"));

    let result = match config.log_format.as_str() {
        "json" => fmt::fmt().with_env_filter(filter).json().try_init(),
        _ => fmt::fmt().with_env_filter(filter).try_init(),
    };
    drop(result);
}

/// Prepare a coop session: set up driver, spawn backend, start servers.
///
/// Returns a [`PreparedSession`] whose [`AppState`] is accessible before
/// calling [`PreparedSession::run`] to enter the session loop.
pub async fn prepare(config: Config) -> anyhow::Result<PreparedSession> {
    init_tracing(&config);

    let shutdown = CancellationToken::new();
    let agent_enum = config.agent_enum()?;

    // 0. Load agent config file if provided.
    let agent_file_config = match config.agent_config {
        Some(ref path) => Some(config::load_agent_config(path)?),
        None => None,
    };
    let stop_config = agent_file_config.as_ref().and_then(|c| c.stop.clone()).unwrap_or_default();
    let start_config = agent_file_config.as_ref().and_then(|c| c.start.clone()).unwrap_or_default();
    let base_settings = agent_file_config.as_ref().and_then(|c| c.settings.clone());
    let mcp_config = agent_file_config.as_ref().and_then(|c| c.mcp.clone());

    // 1. Handle --resume: discover session log and build resume state.
    let (resume_state, resume_log_path) = if let Some(ref resume_hint) = config.resume {
        let log_path = resume::discover_session_log(resume_hint)?
            .ok_or_else(|| anyhow::anyhow!("no session log found for: {resume_hint}"))?;
        info!("resuming from session log: {}", log_path.display());
        let state = resume::parse_resume_state(&log_path)?;
        (Some(state), Some(log_path))
    } else {
        (None, None)
    };

    // 2. Prepare agent session setup. Each agent's setup module produces a
    //    unified `SessionSetup` containing env vars, CLI args, and paths.
    //    In pristine mode, hooks/FIFO are omitted but Tier 2 paths are kept.
    let working_dir = std::env::current_dir()?;
    // Compute coop_url early so it's available for setup.
    // Uses port 0 as placeholder if no HTTP port configured.
    let coop_url_for_setup = format!("http://127.0.0.1:{}", config.port.unwrap_or(0));
    let pristine = config.groom_level()? == GroomLevel::Pristine;

    let base_settings = base_settings.as_ref();
    let mcp_config = mcp_config.as_ref();

    let resume = resume_state.as_ref().zip(resume_log_path.as_deref());
    let setup: Option<SessionSetup> = match agent_enum {
        AgentType::Claude => Some(claude_setup::prepare(
            &working_dir,
            &coop_url_for_setup,
            base_settings,
            mcp_config,
            pristine,
            resume,
        )?),
        AgentType::Gemini => {
            Some(gemini_setup::prepare(&coop_url_for_setup, base_settings, mcp_config, pristine)?)
        }
        _ => None,
    };

    // 3. Build the command with extra args from setup.
    let mut command = config.command.clone();
    if let Some(ref s) = setup {
        command.extend(s.extra_args.clone());
    }

    // 4. Build terminal state early so driver closures can reference its atomics.
    let terminal = Arc::new(TerminalState {
        screen: RwLock::new(Screen::new(config.cols, config.rows)),
        ring: RwLock::new(RingBuffer::new(config.ring_size)),
        ring_total_written: Arc::new(AtomicU64::new(0)),
        child_pid: AtomicU32::new(0),
        exit_status: RwLock::new(None),
    });

    // 5. Build driver (detectors + encoders). For Claude, uses real paths
    //    from the setup so detectors actually activate.
    //    Create raw broadcast channels early so detectors can capture senders.
    let (hook_tx, _) = broadcast::channel(64);
    let (message_tx, _) = broadcast::channel(64);

    let last_message: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
    let sinks = || {
        DetectorSinks::default()
            .with_last_message(Arc::clone(&last_message))
            .with_hook_tx(hook_tx.clone())
            .with_message_tx(message_tx.clone())
    };
    let mut driver = match agent_enum {
        AgentType::Claude => {
            let log_start_offset = resume_state.as_ref().map(|s| s.log_offset).unwrap_or(0);
            build_claude_driver(&config, setup.as_ref(), log_start_offset, sinks())?
        }
        AgentType::Gemini => build_gemini_driver(
            &config,
            setup.as_ref(),
            terminal.child_pid_fn(),
            terminal.ring_total_written_fn(),
            sinks(),
        )?,
        AgentType::Unknown => DriverContext {
            nudge_encoder: None,
            respond_encoder: None,
            detectors: crate::driver::unknown::build_detectors(
                &config,
                terminal.child_pid_fn(),
                terminal.ring_total_written_fn(),
                None,
            )?,
            option_parser: None,
        },
        AgentType::Codex => {
            anyhow::bail!("{agent_enum:?} driver is not yet implemented");
        }
    };

    // Tier 5: Claude screen detector for idle prompt detection.
    if agent_enum == AgentType::Claude {
        driver.detectors.push(Box::new(crate::driver::claude::screen::ClaudeScreenDetector::new(
            &config,
            terminal.snapshot_fn(),
        )));
        driver.detectors.sort_by_key(|d| d.tier());
    }

    // 6. Spawn backend AFTER driver is built (FIFO must exist before child starts).
    let extra_env = setup.as_ref().map(|s| s.env_vars.as_slice()).unwrap_or(&[]);
    let backend: Box<dyn Backend> = if let Some(ref attach_spec) = config.attach {
        let spec: AttachSpec = attach_spec.parse()?;
        match spec {
            AttachSpec::Tmux { session } => {
                Box::new(TmuxBackend::new(session)?.with_poll_interval(config.tmux_poll()))
            }
            AttachSpec::Screen { session: _ } => {
                anyhow::bail!("screen attach is not yet implemented");
            }
        }
    } else {
        if command.is_empty() {
            anyhow::bail!("no command specified");
        }
        Box::new(
            NativePty::spawn(&command, config.cols, config.rows, extra_env)?
                .with_reap_interval(config.reap_poll()),
        )
    };

    // Create shared channels
    let (input_tx, consumer_input_rx) = mpsc::channel(256);
    let (output_tx, _) = broadcast::channel(256);
    let (state_tx, _) = broadcast::channel(64);
    let (prompt_tx, _) = broadcast::channel(64);

    // Switch channel: capacity 1 enforces single-switch-at-a-time.
    let (switch_tx, switch_rx) = mpsc::channel::<SwitchRequest>(1);

    let resolve_url = format!("{coop_url_for_setup}/api/v1/hooks/stop/resolve");
    let stop_state = Arc::new(StopState::new(stop_config, resolve_url));
    let start_state = Arc::new(StartState::new(start_config));
    let switch_state = Arc::new(SwitchState {
        switch_tx,
        session_log_path: RwLock::new(setup.as_ref().and_then(|s| s.session_log_path.clone())),
        base_settings: base_settings.cloned(),
        mcp_config: mcp_config.cloned(),
    });
    let transcript_state = Arc::new(TranscriptState::new(
        setup
            .as_ref()
            .map(|s| s.session_dir.join("transcripts"))
            .unwrap_or_else(|| PathBuf::from("/tmp/coop-transcripts")),
        setup.as_ref().and_then(|s| s.session_log_path.clone()),
    )?);

    let store = Arc::new(Store {
        terminal,
        driver: Arc::new(DriverState {
            agent_state: RwLock::new(AgentState::Starting),
            state_seq: AtomicU64::new(0),
            detection: RwLock::new(DetectionInfo { tier: u8::MAX, cause: String::new() }),
            error: RwLock::new(None),
            last_message,
        }),
        channels: TransportChannels {
            input_tx,
            output_tx,
            state_tx,
            prompt_tx,
            hook_tx,
            message_tx,
        },
        config: SessionSettings {
            started_at: Instant::now(),
            agent: agent_enum,
            auth_token: config.auth_token.clone(),
            nudge_encoder: driver.nudge_encoder,
            respond_encoder: driver.respond_encoder,
            nudge_timeout: config.nudge_timeout(),
            groom: config.groom_level()?,
        },
        lifecycle: LifecycleState {
            shutdown: shutdown.clone(),
            ws_client_count: AtomicI32::new(0),
            bytes_written: AtomicU64::new(0),
        },
        ready: Arc::new(AtomicBool::new(false)),
        input_gate: Arc::new(crate::transport::state::InputGate::new(config.input_delay())),
        stop: stop_state,
        switch: switch_state,
        start: start_state,
        transcript: transcript_state,
        input_activity: Arc::new(tokio::sync::Notify::new()),
    });

    // Spawn HTTP server
    if let Some(port) = config.port {
        let router = build_router(Arc::clone(&store));
        let addr = format!("{}:{}", config.host, port);
        let listener = TcpListener::bind(&addr).await?;
        info!("HTTP listening on {addr}");
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let result =
                axum::serve(listener, router).with_graceful_shutdown(sd.cancelled_owned()).await;
            if let Err(e) = result {
                error!("HTTP server error: {e}");
            }
        });
    }

    // Spawn Unix socket server
    if let Some(ref socket_path) = config.socket {
        let router = build_router(Arc::clone(&store));
        let path = socket_path.clone();
        // Remove stale socket
        let _ = std::fs::remove_file(&path);
        let uds_listener = tokio::net::UnixListener::bind(&path)?;
        info!("Unix socket listening on {path}");
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let mut make_svc = router.into_make_service();
            loop {
                tokio::select! {
                    _ = sd.cancelled() => break,
                    accept = uds_listener.accept() => {
                        match accept {
                            Ok((stream, _)) => {
                                // IntoMakeService implements Service<T> for any T
                                let svc_future = <_ as tower::Service<_>>::call(&mut make_svc, ());
                                tokio::spawn(async move {
                                    let Ok(svc) = svc_future.await;
                                    let io = hyper_util::rt::TokioIo::new(stream);
                                    let hyper_svc = hyper_util::service::TowerToHyperService::new(svc);
                                    let _ = hyper_util::server::conn::auto::Builder::new(
                                        hyper_util::rt::TokioExecutor::new(),
                                    )
                                    .serve_connection_with_upgrades(io, hyper_svc)
                                    .await;
                                });
                            }
                            Err(e) => {
                                tracing::debug!("unix socket accept error: {e}");
                            }
                        }
                    }
                }
            }
        });
    }

    // Spawn gRPC server
    if let Some(grpc_port) = config.port_grpc {
        let grpc = CoopGrpc::new(Arc::clone(&store));
        let addr = format!("{}:{}", config.host, grpc_port).parse()?;
        info!("gRPC listening on {addr}");
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let result = grpc.into_router().serve_with_shutdown(addr, sd.cancelled_owned()).await;
            if let Err(e) = result {
                error!("gRPC server error: {e}");
            }
        });
    }

    // Spawn health probe
    if let Some(health_port) = config.port_health {
        let health_router = build_health_router(Arc::clone(&store));
        let addr = format!("{}:{}", config.host, health_port);
        let listener = TcpListener::bind(&addr).await?;
        info!("health probe listening on {addr}");
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let result = axum::serve(listener, health_router)
                .with_graceful_shutdown(sd.cancelled_owned())
                .await;
            if let Err(e) = result {
                error!("health server error: {e}");
            }
        });
    }

    // Spawn signal handler
    {
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
            let mut sigint =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).ok();

            tokio::select! {
                _ = async {
                    if let Some(ref mut s) = sigterm { s.recv().await } else { std::future::pending().await }
                } => {
                    info!("received SIGTERM");
                    sd.cancel();
                }
                _ = async {
                    if let Some(ref mut s) = sigint { s.recv().await } else { std::future::pending().await }
                } => {
                    info!("received SIGINT");
                    sd.cancel();
                }
            }
        });
    }

    // Build session (but don't run yet — caller may need store first).
    // Session::run borrows the receivers via &mut; PreparedSession retains
    // ownership so they survive across switch iterations.
    let mut session_config = SessionConfig::new(Arc::clone(&store), backend)
        .with_detectors(driver.detectors)
        .with_shutdown(shutdown);
    if let Some(parser) = driver.option_parser {
        session_config = session_config.with_option_parser(parser);
    }
    let session = Session::new(&config, session_config);

    // `setup` is intentionally dropped here — session artifacts live in
    // persistent XDG_STATE_HOME directories, not ephemeral temp dirs.
    drop(setup);

    Ok(PreparedSession { store, session: Some(session), config, consumer_input_rx, switch_rx })
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
