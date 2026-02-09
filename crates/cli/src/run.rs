// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Top-level session runner — shared by `main` and integration tests.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use tracing_subscriber::EnvFilter;

use crate::config::{self, Config};
use crate::driver::claude::resume;
use crate::driver::claude::setup::{self as claude_setup, ClaudeSessionSetup};
use crate::driver::claude::{ClaudeDriver, ClaudeDriverConfig};
use crate::driver::gemini::setup::{self as gemini_setup, GeminiSessionSetup};
use crate::driver::gemini::{GeminiDriver, GeminiDriverConfig};
use crate::driver::AgentType;
use crate::driver::{AgentState, Detector, NudgeEncoder, RespondEncoder};
use crate::pty::attach::{AttachSpec, TmuxBackend};
use crate::pty::spawn::NativePty;
use crate::pty::Backend;
use crate::ring::RingBuffer;
use crate::screen::Screen;
use crate::session::Session;
use crate::stop::StopState;
use crate::transport::grpc::CoopGrpc;
use crate::transport::state::{
    DriverState, LifecycleState, SessionSettings, TerminalState, TransportChannels,
};
use crate::transport::{build_health_router, build_router, AppState};

/// Result of a completed session.
pub struct RunResult {
    pub status: crate::driver::ExitStatus,
    pub app_state: Arc<AppState>,
}

/// A fully-prepared session ready to run.
///
/// Returned by [`prepare`] so callers (e.g. integration tests) can access
/// [`AppState`] — including broadcast channels and the shutdown token — before
/// the blocking session loop starts.
pub struct PreparedSession {
    pub app_state: Arc<AppState>,
    session: Session,
    config: Config,
}

impl PreparedSession {
    /// Run the session loop to completion.
    pub async fn run(self) -> anyhow::Result<RunResult> {
        let status = self.session.run(&self.config).await?;
        Ok(RunResult {
            status,
            app_state: self.app_state,
        })
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
    let stop_config = agent_file_config
        .as_ref()
        .and_then(|c| c.stop.clone())
        .unwrap_or_default();

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

    // 2. Prepare Claude session setup (creates FIFO pipe path, writes settings,
    //    computes extra args). Must happen BEFORE backend spawn so the child
    //    finds the FIFO and settings file on startup.
    let working_dir = std::env::current_dir()?;
    // Compute coop_url early so it's available for Claude setup.
    // Uses port 0 as placeholder if no HTTP port configured.
    let coop_url_for_setup = format!("http://127.0.0.1:{}", config.port.unwrap_or(0));

    let claude_setup: Option<ClaudeSessionSetup> = if agent_enum == AgentType::Claude {
        let setup = if let (Some(ref state), Some(ref log_path)) = (&resume_state, &resume_log_path)
        {
            claude_setup::prepare_claude_resume(state, log_path, &coop_url_for_setup)?
        } else {
            claude_setup::prepare_claude_session(&working_dir, &coop_url_for_setup)?
        };
        Some(setup)
    } else {
        None
    };

    // 2b. Prepare Gemini session setup (creates FIFO pipe path, writes settings).
    let gemini_setup: Option<GeminiSessionSetup> = if agent_enum == AgentType::Gemini {
        Some(gemini_setup::prepare_gemini_session(&working_dir)?)
    } else {
        None
    };

    // 3. Build the command with extra args from setup.
    let mut command = config.command.clone();
    if let Some(ref setup) = claude_setup {
        command.extend(setup.extra_args.clone());
    }
    if let Some(ref setup) = gemini_setup {
        command.extend(setup.extra_args.clone());
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
    let log_start_offset = resume_state.as_ref().map(|s| s.log_offset).unwrap_or(0);
    let pid_terminal = Arc::clone(&terminal);
    let rtw_for_driver = Arc::clone(&terminal.ring_total_written);
    let (nudge_encoder, respond_encoder, detectors) = build_driver(
        &config,
        agent_enum,
        claude_setup.as_ref(),
        gemini_setup.as_ref(),
        Arc::new(move || {
            let v = pid_terminal
                .child_pid
                .load(std::sync::atomic::Ordering::Relaxed);
            if v == 0 {
                None
            } else {
                Some(v)
            }
        }),
        Arc::new(move || rtw_for_driver.load(std::sync::atomic::Ordering::Relaxed)),
        log_start_offset,
    )?;

    // 6. Spawn backend AFTER driver is built (FIFO must exist before child starts).
    let extra_env: Vec<(String, String)> = claude_setup
        .as_ref()
        .map(|s| s.env_vars.clone())
        .or_else(|| gemini_setup.as_ref().map(|s| s.env_vars.clone()))
        .unwrap_or_default();
    let extra_env = extra_env.as_slice();
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
                .with_reap_interval(config.pty_reap()),
        )
    };

    // Create shared channels
    let (input_tx, consumer_input_rx) = mpsc::channel(256);
    let (output_tx, _) = broadcast::channel(256);
    let (state_tx, _) = broadcast::channel(64);

    let resolve_url = format!("{coop_url_for_setup}/api/v1/hooks/stop/resolve");
    let stop_state = Arc::new(StopState::new(stop_config, resolve_url));

    let app_state = Arc::new(AppState {
        terminal,
        driver: Arc::new(DriverState {
            agent_state: RwLock::new(AgentState::Starting),
            state_seq: AtomicU64::new(0),
            detection_tier: AtomicU8::new(u8::MAX),
            idle_grace_deadline: Arc::new(parking_lot::Mutex::new(None)),
            error_detail: RwLock::new(None),
            error_category: RwLock::new(None),
        }),
        channels: TransportChannels {
            input_tx,
            output_tx,
            state_tx,
        },
        config: SessionSettings {
            started_at: Instant::now(),
            agent: agent_enum,
            auth_token: config.auth_token.clone(),
            nudge_encoder,
            respond_encoder,
            idle_grace_duration: Duration::from_secs(config.idle_grace),
        },
        lifecycle: LifecycleState {
            shutdown: shutdown.clone(),
            ws_client_count: AtomicI32::new(0),
            bytes_written: AtomicU64::new(0),
        },
        ready: Arc::new(AtomicBool::new(false)),
        nudge_mutex: Arc::new(tokio::sync::Mutex::new(())),
        stop: stop_state,
    });

    // Spawn HTTP server
    if let Some(port) = config.port {
        let router = build_router(Arc::clone(&app_state));
        let addr = format!("{}:{}", config.host, port);
        let listener = TcpListener::bind(&addr).await?;
        info!("HTTP listening on {addr}");
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(sd.cancelled_owned())
                .await;
            if let Err(e) = result {
                error!("HTTP server error: {e}");
            }
        });
    }

    // Spawn Unix socket server
    if let Some(ref socket_path) = config.socket {
        let router = build_router(Arc::clone(&app_state));
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
    if let Some(grpc_port) = config.grpc_port {
        let grpc = CoopGrpc::new(Arc::clone(&app_state));
        let addr = format!("{}:{}", config.host, grpc_port).parse()?;
        info!("gRPC listening on {addr}");
        let sd = shutdown.clone();
        tokio::spawn(async move {
            let result = grpc
                .into_router()
                .serve_with_shutdown(addr, sd.cancelled_owned())
                .await;
            if let Err(e) = result {
                error!("gRPC server error: {e}");
            }
        });
    }

    // Spawn health probe
    if let Some(health_port) = config.health_port {
        let health_router = build_health_router(Arc::clone(&app_state));
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

    // Build session (but don't run yet — caller may need app_state first)
    let session = Session::new(
        &config,
        crate::session::SessionConfig::new(Arc::clone(&app_state), backend, consumer_input_rx)
            .with_detectors(detectors)
            .with_shutdown(shutdown),
    );

    Ok(PreparedSession {
        app_state,
        session,
        config,
    })
}

type DriverComponents = (
    Option<Arc<dyn NudgeEncoder>>,
    Option<Arc<dyn RespondEncoder>>,
    Vec<Box<dyn Detector>>,
);

fn build_driver(
    config: &Config,
    agent: AgentType,
    claude_setup: Option<&ClaudeSessionSetup>,
    gemini_setup: Option<&GeminiSessionSetup>,
    child_pid_fn: Arc<dyn Fn() -> Option<u32> + Send + Sync>,
    ring_total_written_fn: Arc<dyn Fn() -> u64 + Send + Sync>,
    log_start_offset: u64,
) -> anyhow::Result<DriverComponents> {
    match agent {
        AgentType::Claude => {
            let driver = ClaudeDriver::new(ClaudeDriverConfig {
                session_log_path: claude_setup.map(|s| s.session_log_path.clone()),
                hook_pipe_path: claude_setup.map(|s| s.hook_pipe_path.clone()),
                stdout_rx: None,
                log_start_offset,
                log_poll: config.log_poll(),
                feedback_delay: config.feedback_delay(),
                input_delay: config.keyboard_delay(),
            })?;
            let nudge: Arc<dyn NudgeEncoder> = Arc::new(driver.nudge);
            let respond: Arc<dyn RespondEncoder> = Arc::new(driver.respond);
            let detectors = driver.detectors;
            Ok((Some(nudge), Some(respond), detectors))
        }
        AgentType::Gemini => {
            let driver = GeminiDriver::new(GeminiDriverConfig {
                hook_pipe_path: gemini_setup.map(|s| s.hook_pipe_path.clone()),
                stdout_rx: None,
                feedback_delay: config.feedback_delay(),
            })?;
            let nudge: Arc<dyn NudgeEncoder> = Arc::new(driver.nudge);
            let respond: Arc<dyn RespondEncoder> = Arc::new(driver.respond);
            let detectors = driver.detectors;
            Ok((Some(nudge), Some(respond), detectors))
        }
        AgentType::Unknown => {
            let detectors = crate::driver::unknown::build_detectors(
                config,
                child_pid_fn,
                ring_total_written_fn,
                None,
            )?;
            Ok((None, None, detectors))
        }
        AgentType::Codex => {
            anyhow::bail!("{agent:?} driver is not yet implemented");
        }
    }
}
