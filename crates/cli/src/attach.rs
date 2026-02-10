// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! `coop attach` — interactive terminal client for a running coop server.
//!
//! Connects to a coop server via WebSocket, puts the local terminal in raw
//! mode, and proxies I/O between the user's terminal and the remote session.
//! Detach with Ctrl+] (0x1d).
//!
//! When a statusline is configured (via `--statusline-cmd` or the default
//! built-in), the bottom row of the terminal is reserved for a status bar
//! using DECSTBM scroll region margins.

use std::io::Write;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use nix::sys::termios;
use tokio::sync::mpsc;

use crate::transport::ws::{ClientMessage, ServerMessage};

/// Detach key: Ctrl+] (ASCII 0x1d), same as telnet / docker attach.
const DETACH_KEY: u8 = 0x1d;

/// Default statusline refresh interval in seconds.
const DEFAULT_STATUSLINE_INTERVAL: u64 = 5;

/// Ping keepalive interval.
const PING_INTERVAL: Duration = Duration::from_secs(30);

struct StatuslineConfig {
    /// Shell command to run for statusline content. None = built-in.
    cmd: Option<String>,
    /// Refresh interval.
    interval: Duration,
    /// Whether statusline is enabled at all.
    enabled: bool,
}

impl StatuslineConfig {
    fn from_args(args: &[String]) -> Self {
        let no_statusline = args.iter().any(|a| a == "--no-statusline");

        let cmd = find_arg_value(args, "--statusline-cmd")
            .or_else(|| std::env::var("COOP_STATUSLINE_CMD").ok());

        let interval_secs: u64 = find_arg_value(args, "--statusline-interval")
            .or_else(|| std::env::var("COOP_STATUSLINE_INTERVAL").ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_STATUSLINE_INTERVAL);

        Self { cmd, interval: Duration::from_secs(interval_secs), enabled: !no_statusline }
    }
}

/// Find the value for a `--key value` or `--key=value` style arg.
fn find_arg_value(args: &[String], key: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == key {
            return iter.next().cloned();
        }
        if let Some(val) = arg.strip_prefix(&format!("{key}=")) {
            return Some(val.to_owned());
        }
    }
    None
}

/// Mutable state tracked for statusline rendering.
struct AttachState {
    agent_state: String,
    cols: u16,
    rows: u16,
    started: Instant,
}

impl AttachState {
    fn new(cols: u16, rows: u16) -> Self {
        Self { agent_state: "unknown".to_owned(), cols, rows, started: Instant::now() }
    }

    fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

/// RAII guard that restores the original terminal attributes on drop.
///
/// Stores a raw fd (stdin) and the original termios state. The fd is valid
/// for the lifetime of the process (stdin never closes), so this is safe.
struct RawModeGuard {
    fd: i32,
    original: termios::Termios,
}

impl RawModeGuard {
    fn enter() -> anyhow::Result<Self> {
        let fd = std::io::stdin().as_raw_fd();
        let borrowed = borrow_fd(fd);
        let original = termios::tcgetattr(borrowed)?;
        let mut raw = original.clone();
        termios::cfmakeraw(&mut raw);
        termios::tcsetattr(borrowed, termios::SetArg::TCSAFLUSH, &raw)?;
        Ok(Self { fd, original })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let borrowed = borrow_fd(self.fd);
        let _ = termios::tcsetattr(borrowed, termios::SetArg::TCSAFLUSH, &self.original);
    }
}

/// Create a `BorrowedFd` from a raw fd that we know is valid.
fn borrow_fd(fd: i32) -> BorrowedFd<'static> {
    // SAFETY: stdin fd 0 is valid for the lifetime of the process.
    #[allow(unsafe_code)]
    unsafe {
        BorrowedFd::borrow_raw(fd)
    }
}

fn terminal_size() -> Option<(u16, u16)> {
    let fd = std::io::stdout().as_raw_fd();
    let mut ws = nix::libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCGWINSZ ioctl reads terminal size into a winsize struct.
    // The fd is stdout which is valid, and ws is a properly-initialized stack
    // variable with the correct layout for this ioctl.
    #[allow(unsafe_code)]
    let ret = unsafe { nix::libc::ioctl(fd, nix::libc::TIOCGWINSZ, &mut ws) };
    if ret == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
        Some((ws.ws_col, ws.ws_row))
    } else {
        None
    }
}

/// Set the scroll region to rows 1..content_rows (leaving the last row free
/// for the statusline). Moves cursor to home position.
fn set_scroll_region(stdout: &mut std::io::Stdout, content_rows: u16) {
    // ESC[1;Nr — set scroll region. ESC[H — move cursor to home.
    let _ = write!(stdout, "\x1b[1;{content_rows}r\x1b[H");
    let _ = stdout.flush();
}

/// Reset the scroll region to full terminal.
fn reset_scroll_region(stdout: &mut std::io::Stdout) {
    let _ = write!(stdout, "\x1b[r");
    let _ = stdout.flush();
}

/// Render the statusline on the bottom row of the terminal.
fn render_statusline(stdout: &mut std::io::Stdout, content: &str, cols: u16, rows: u16) {
    // Truncate to column width at a valid char boundary.
    let max = cols as usize;
    let truncated =
        if content.len() > max { &content[..content.floor_char_boundary(max)] } else { content };
    // Save cursor, move to last row col 1, reverse video, write padded content, restore.
    let _ = write!(
        stdout,
        "\x1b7\x1b[{rows};1H\x1b[7m{truncated:<width$}\x1b[0m\x1b8",
        width = cols as usize
    );
    let _ = stdout.flush();
}

/// Build the default built-in statusline string.
fn builtin_statusline(state: &AttachState) -> String {
    format!(
        " [coop] {} | {}s | {}x{}",
        state.agent_state,
        state.uptime_secs(),
        state.cols,
        state.rows
    )
}

/// Run a shell command and capture its stdout as a statusline string.
async fn run_statusline_cmd(cmd: &str, state: &AttachState) -> String {
    // Expand template variables.
    let expanded = cmd
        .replace("{state}", &state.agent_state)
        .replace("{cols}", &state.cols.to_string())
        .replace("{rows}", &state.rows.to_string())
        .replace("{uptime}", &state.uptime_secs().to_string());

    let output = tokio::process::Command::new("sh")
        .args(["-c", &expanded])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        _ => format!(" [coop] statusline cmd failed: {cmd}"),
    }
}

/// Run the `coop attach` subcommand. Returns a process exit code.
///
/// `args` contains everything after "attach" on the command line.
///
/// This is async because the attach loop uses tokio for WebSocket I/O,
/// signal handling, and timers. It must be called from within a tokio
/// runtime (e.g. from `#[tokio::main]` in main.rs).
pub async fn run(args: &[String]) -> i32 {
    // Parse a simple --help flag.
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("Usage: coop attach [URL] [OPTIONS]");
        eprintln!();
        eprintln!("Connect to a running coop server and attach an interactive terminal.");
        eprintln!("Detach with Ctrl+].");
        eprintln!();
        eprintln!("URL defaults to COOP_URL env var. Auth via COOP_AUTH_TOKEN env var.");
        eprintln!();
        eprintln!("Options:");
        eprintln!("  --statusline-cmd CMD    Shell command for statusline (default: built-in)");
        eprintln!("  --statusline-interval N Refresh interval in seconds (default: 5)");
        eprintln!("  --no-statusline         Disable the statusline");
        return 0;
    }

    let statusline_cfg = StatuslineConfig::from_args(args);

    // First positional arg (not starting with --) is the URL.
    let coop_url = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .or_else(|| std::env::var("COOP_URL").ok());

    let coop_url = match coop_url {
        Some(u) => u,
        None => {
            eprintln!("error: COOP_URL is not set and no URL argument provided");
            return 2;
        }
    };

    let auth_token = std::env::var("COOP_AUTH_TOKEN").ok();

    attach(&coop_url, auth_token.as_deref(), &statusline_cfg).await
}

async fn attach(coop_url: &str, auth_token: Option<&str>, sl_cfg: &StatuslineConfig) -> i32 {
    // Convert HTTP URL to WebSocket URL.
    // Use mode=all when statusline is enabled so we get state_change events.
    let mode = if sl_cfg.enabled { "all" } else { "raw" };
    let base = coop_url.trim_end_matches('/');
    let ws_url = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}/ws?mode={mode}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}/ws?mode={mode}")
    } else {
        format!("ws://{base}/ws?mode={mode}")
    };

    // Auth via message only — no token in URL to avoid leakage in logs.

    // Connect WebSocket.
    let (ws_stream, _response) = match tokio_tungstenite::connect_async(&ws_url).await {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("error: WebSocket connection failed: {e}");
            return 1;
        }
    };

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Enter raw mode.
    let raw_guard = match RawModeGuard::enter() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: failed to enter raw mode: {e}");
            return 1;
        }
    };

    // Install a panic hook to restore the terminal even on unwind.
    let terminal_restored = Arc::new(AtomicBool::new(false));
    {
        let restored = Arc::clone(&terminal_restored);
        let raw_termios: nix::libc::termios = raw_guard.original.clone().into();
        let fd = raw_guard.fd;
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !restored.swap(true, Ordering::SeqCst) {
                // SAFETY: Restoring terminal attributes in panic hook; fd is
                // stdin which remains valid for the lifetime of the process.
                #[allow(unsafe_code)]
                unsafe {
                    nix::libc::tcsetattr(fd, nix::libc::TCSAFLUSH, &raw_termios);
                }
            }
            prev_hook(info);
        }));
    }

    let mut stdout = std::io::stdout();

    // Determine initial terminal size.
    let (init_cols, init_rows) = terminal_size().unwrap_or((80, 24));
    let mut state = AttachState::new(init_cols, init_rows);

    // If statusline is enabled, set up scroll region and report smaller size.
    let sl_active = sl_cfg.enabled && init_rows > 2;
    if sl_active {
        let content_rows = init_rows - 1;
        set_scroll_region(&mut stdout, content_rows);
        let resize = ClientMessage::Resize { cols: init_cols, rows: content_rows };
        let _ = send_msg(&mut ws_tx, &resize).await;
    } else {
        let resize = ClientMessage::Resize { cols: init_cols, rows: init_rows };
        let _ = send_msg(&mut ws_tx, &resize).await;
    }

    // Send initial Replay to catch up on any missed output.
    let replay = ClientMessage::Replay { offset: 0 };
    if let Err(e) = send_msg(&mut ws_tx, &replay).await {
        reset_scroll_region(&mut stdout);
        drop(raw_guard);
        eprintln!("error: failed to send replay request: {e}");
        return 1;
    }

    // Request current state so we can populate the statusline immediately.
    if sl_active {
        let _ = send_msg(&mut ws_tx, &ClientMessage::StateRequest {}).await;
    }

    // Auth via message if token provided (no query param — avoids log leakage).
    if let Some(token) = auth_token {
        let auth = ClientMessage::Auth { token: token.to_owned() };
        let _ = send_msg(&mut ws_tx, &auth).await;
    }

    // Render initial statusline.
    if sl_active {
        let content = match &sl_cfg.cmd {
            Some(cmd) => run_statusline_cmd(cmd, &state).await,
            None => builtin_statusline(&state),
        };
        render_statusline(&mut stdout, &content, state.cols, state.rows);
    }

    // Spawn a blocking thread to read stdin.
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        use std::io::Read;
        let stdin = std::io::stdin();
        let mut handle = stdin.lock();
        let mut buf = [0u8; 4096];
        loop {
            match handle.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if stdin_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // SIGWINCH handler for terminal resize.
    let mut sigwinch =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()).ok();

    // Statusline refresh timer.
    let mut sl_interval = tokio::time::interval(sl_cfg.interval);
    sl_interval.tick().await; // Consume the immediate first tick.

    // Ping keepalive timer.
    let mut ping_interval = tokio::time::interval(PING_INTERVAL);
    ping_interval.tick().await; // Consume the immediate first tick.

    let mut exit_code: i32 = 0;

    // Main event loop.
    loop {
        tokio::select! {
            // Incoming WebSocket messages.
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                        match serde_json::from_str::<ServerMessage>(&text) {
                            Ok(ServerMessage::Output { data, .. }) => {
                                if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&data) {
                                    let _ = stdout.write_all(&decoded);
                                    let _ = stdout.flush();
                                }
                            }
                            Ok(ServerMessage::Exit { code, .. }) => {
                                exit_code = code.unwrap_or(0);
                                break;
                            }
                            Ok(ServerMessage::Error { code, message }) => {
                                reset_scroll_region(&mut stdout);
                                drop(raw_guard);
                                eprintln!("\r\ncoop attach: server error: [{code}] {message}");
                                return 1;
                            }
                            Ok(ServerMessage::StateChange { next, .. }) => {
                                state.agent_state = next;
                                // Immediately refresh statusline on state change.
                                if sl_active {
                                    let content = match &sl_cfg.cmd {
                                        Some(cmd) => run_statusline_cmd(cmd, &state).await,
                                        None => builtin_statusline(&state),
                                    };
                                    render_statusline(&mut stdout, &content, state.cols, state.rows);
                                }
                            }
                            Ok(_) => {}
                            Err(_) => {}
                        }
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => {
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => {
                        break;
                    }
                }
            }

            // Local stdin input.
            data = stdin_rx.recv() => {
                match data {
                    Some(bytes) => {
                        // Check for detach key; send bytes before it, then break.
                        if let Some(pos) = bytes.iter().position(|&b| b == DETACH_KEY) {
                            if pos > 0 {
                                let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes[..pos]);
                                let msg = ClientMessage::InputRaw { data: encoded };
                                let _ = send_msg(&mut ws_tx, &msg).await;
                            }
                            break;
                        }
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        let msg = ClientMessage::InputRaw { data: encoded };
                        if send_msg(&mut ws_tx, &msg).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }

            // Terminal resize.
            _ = async {
                match sigwinch.as_mut() {
                    Some(s) => { s.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                if let Some((cols, rows)) = terminal_size() {
                    state.cols = cols;
                    state.rows = rows;

                    if sl_active && rows > 2 {
                        // Reset scroll region first, then set new one.
                        reset_scroll_region(&mut stdout);
                        let content_rows = rows - 1;
                        set_scroll_region(&mut stdout, content_rows);
                        let msg = ClientMessage::Resize { cols, rows: content_rows };
                        let _ = send_msg(&mut ws_tx, &msg).await;
                        // Re-render statusline for new dimensions.
                        let content = match &sl_cfg.cmd {
                            Some(cmd) => run_statusline_cmd(cmd, &state).await,
                            None => builtin_statusline(&state),
                        };
                        render_statusline(&mut stdout, &content, cols, rows);
                    } else {
                        let msg = ClientMessage::Resize { cols, rows };
                        let _ = send_msg(&mut ws_tx, &msg).await;
                    }
                }
            }

            // Statusline refresh timer.
            _ = sl_interval.tick(), if sl_active => {
                let content = match &sl_cfg.cmd {
                    Some(cmd) => run_statusline_cmd(cmd, &state).await,
                    None => builtin_statusline(&state),
                };
                render_statusline(&mut stdout, &content, state.cols, state.rows);
            }

            // Ping keepalive.
            _ = ping_interval.tick() => {
                let _ = send_msg(&mut ws_tx, &ClientMessage::Ping {}).await;
            }
        }
    }

    // Send close frame before dropping.
    let _ = ws_tx.send(tokio_tungstenite::tungstenite::Message::Close(None)).await;

    // Clean up: reset scroll region, restore terminal, print detach message.
    if sl_active {
        reset_scroll_region(&mut stdout);
    }
    drop(raw_guard);
    eprintln!("\r\ndetached from coop session.");
    exit_code
}

/// Serialize and send a JSON text message over WebSocket.
async fn send_msg<S>(tx: &mut S, msg: &ClientMessage) -> Result<(), String>
where
    S: SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin,
{
    let text = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    tx.send(tokio_tungstenite::tungstenite::Message::Text(text))
        .await
        .map_err(|_| "WebSocket send failed".to_owned())
}

#[cfg(test)]
#[path = "attach_tests.rs"]
mod tests;
