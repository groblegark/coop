// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! `coop attach` — interactive terminal client for a running coop server.
//!
//! Connects to a coop server via WebSocket, puts the local terminal in raw
//! mode, and proxies I/O between the user's terminal and the remote session.
//! Detach with Ctrl+] (0x1d).

use std::io::Write;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use nix::sys::termios;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Detach key: Ctrl+] (ASCII 0x1d), same as telnet / docker attach.
const DETACH_KEY: u8 = 0x1d;

// ---------------------------------------------------------------------------
// Wire types (client-side copies — kept minimal)
// ---------------------------------------------------------------------------

/// Messages we send to the coop server.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    InputRaw { data: String },
    Resize { cols: u16, rows: u16 },
    Replay { offset: u64 },
    Auth { token: String },
}

/// Messages we receive from the coop server.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMsg {
    Output {
        data: String,
        #[serde(default)]
        #[allow(dead_code)]
        offset: u64,
    },
    Exit {
        code: Option<i32>,
        #[allow(dead_code)]
        signal: Option<i32>,
    },
    Error {
        code: String,
        message: String,
    },
    Resize {
        #[allow(dead_code)]
        cols: u16,
        #[allow(dead_code)]
        rows: u16,
    },
    Pong {},
    // Ignore state_change, screen, stop — we only subscribe to raw mode.
    #[serde(other)]
    Other,
}

// ---------------------------------------------------------------------------
// Terminal raw mode via nix
// ---------------------------------------------------------------------------

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
        // SAFETY: stdin fd is valid for the process lifetime. We create a
        // temporary BorrowedFd for the nix call.
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
///
/// SAFETY: The caller must ensure `fd` is a valid, open file descriptor.
/// In this module we only use this with stdin (fd 0), which is always valid.
fn borrow_fd(fd: i32) -> BorrowedFd<'static> {
    // SAFETY: stdin fd 0 is valid for the lifetime of the process.
    #[allow(unsafe_code)]
    unsafe {
        BorrowedFd::borrow_raw(fd)
    }
}

// ---------------------------------------------------------------------------
// Terminal size
// ---------------------------------------------------------------------------

fn terminal_size() -> Option<(u16, u16)> {
    let fd = std::io::stdout().as_raw_fd();
    let mut ws = nix::libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ ioctl reads terminal size into a winsize struct.
    // stdout fd is always valid.
    #[allow(unsafe_code)]
    let ret = unsafe { nix::libc::ioctl(fd, nix::libc::TIOCGWINSZ, &mut ws) };
    if ret == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
        Some((ws.ws_col, ws.ws_row))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run the `coop attach` subcommand. Returns a process exit code.
///
/// `args` contains everything after "attach" on the command line.
pub fn run(args: &[String]) -> i32 {
    // Parse a simple --help flag.
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("Usage: coop attach [URL]");
        eprintln!();
        eprintln!("Connect to a running coop server and attach an interactive terminal.");
        eprintln!("Detach with Ctrl+].");
        eprintln!();
        eprintln!("URL defaults to COOP_URL env var. Auth via COOP_AUTH_TOKEN env var.");
        return 0;
    }

    let coop_url = if let Some(url) = args.first() {
        url.clone()
    } else {
        match std::env::var("COOP_URL") {
            Ok(u) => u,
            Err(_) => {
                eprintln!("error: COOP_URL is not set and no URL argument provided");
                return 2;
            }
        }
    };

    let auth_token = std::env::var("COOP_AUTH_TOKEN").ok();

    // Build a single-threaded tokio runtime.
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to create runtime: {e}");
            return 1;
        }
    };

    rt.block_on(attach_inner(&coop_url, auth_token.as_deref()))
}

// ---------------------------------------------------------------------------
// Async core
// ---------------------------------------------------------------------------

async fn attach_inner(coop_url: &str, auth_token: Option<&str>) -> i32 {
    // Convert HTTP URL to WebSocket URL.
    let base = coop_url.trim_end_matches('/');
    let ws_url = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}/ws?mode=raw")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}/ws?mode=raw")
    } else {
        // Assume it's already a host:port.
        format!("ws://{base}/ws?mode=raw")
    };

    // Append auth token to query string if provided.
    let ws_url = match auth_token {
        Some(token) => format!("{ws_url}&token={token}"),
        None => ws_url,
    };

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
    // We extract the raw libc::termios to avoid the Termios RefCell (not Sync).
    let terminal_restored = Arc::new(AtomicBool::new(false));
    {
        let restored = Arc::clone(&terminal_restored);
        let raw_termios: nix::libc::termios = raw_guard.original.clone().into();
        let fd = raw_guard.fd;
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !restored.swap(true, Ordering::SeqCst) {
                // SAFETY: Direct libc call to restore terminal. fd 0 (stdin)
                // is always valid.
                #[allow(unsafe_code)]
                unsafe {
                    nix::libc::tcsetattr(fd, nix::libc::TCSAFLUSH, &raw_termios);
                }
            }
            prev_hook(info);
        }));
    }

    // Send initial Replay to catch up on any missed output.
    let replay = ClientMsg::Replay { offset: 0 };
    if let Err(e) = send_msg(&mut ws_tx, &replay).await {
        drop(raw_guard);
        eprintln!("error: failed to send replay request: {e}");
        return 1;
    }

    // Send initial terminal size.
    if let Some((cols, rows)) = terminal_size() {
        let resize = ClientMsg::Resize { cols, rows };
        let _ = send_msg(&mut ws_tx, &resize).await;
    }

    // Auth via message if token provided (belt-and-suspenders with query param).
    if let Some(token) = auth_token {
        let auth = ClientMsg::Auth { token: token.to_owned() };
        let _ = send_msg(&mut ws_tx, &auth).await;
    }

    // Spawn a blocking thread to read stdin, since tokio stdin on raw mode
    // can be problematic.
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

    let mut stdout = std::io::stdout();
    let mut exit_code: i32 = 0;

    // Main event loop.
    loop {
        tokio::select! {
            // Incoming WebSocket messages.
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                        match serde_json::from_str::<ServerMsg>(&text) {
                            Ok(ServerMsg::Output { data, .. }) => {
                                if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&data) {
                                    let _ = stdout.write_all(&decoded);
                                    let _ = stdout.flush();
                                }
                            }
                            Ok(ServerMsg::Exit { code, .. }) => {
                                exit_code = code.unwrap_or(0);
                                break;
                            }
                            Ok(ServerMsg::Error { code, message }) => {
                                drop(raw_guard);
                                eprintln!("\r\ncoop attach: server error: [{code}] {message}");
                                return 1;
                            }
                            Ok(ServerMsg::Pong {} | ServerMsg::Resize { .. } | ServerMsg::Other) => {}
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
                        if bytes.contains(&DETACH_KEY) {
                            break;
                        }
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        let msg = ClientMsg::InputRaw { data: encoded };
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
                    let msg = ClientMsg::Resize { cols, rows };
                    let _ = send_msg(&mut ws_tx, &msg).await;
                }
            }
        }
    }

    drop(raw_guard);
    eprintln!("\r\ndetached from coop session.");
    exit_code
}

/// Serialize and send a JSON text message over WebSocket.
async fn send_msg<S>(tx: &mut S, msg: &ClientMsg) -> Result<(), String>
where
    S: SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin,
{
    let text = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    tx.send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
        .await
        .map_err(|_| "WebSocket send failed".to_owned())
}

#[cfg(test)]
#[path = "attach_tests.rs"]
mod tests;
