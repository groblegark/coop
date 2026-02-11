// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::ffi::CString;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use bytes::Bytes;
use nix::libc;
use nix::pty::{forkpty, ForkptyResult, Winsize};
use nix::sys::signal::{kill, SigHandler, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{execvp, Pid};
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;

use super::nbio::{read_chunk, set_nonblocking, write_all, PtyFd};
use super::{Backend, BackendInput};
use crate::driver::ExitStatus;

/// Native PTY backend that spawns a child process via `forkpty`.
pub struct NativePty {
    master: AsyncFd<PtyFd>,
    child_pid: Pid,
    cols: Arc<AtomicU16>,
    rows: Arc<AtomicU16>,
    reap_interval: Duration,
}

impl NativePty {
    /// Spawn a child process on a new PTY.
    ///
    /// `command` must have at least one element (the program to run).
    /// `extra_env` sets additional environment variables in the child.
    // forkpty requires unsafe: post-fork child is partially initialized
    #[allow(unsafe_code)]
    pub fn spawn(
        command: &[String],
        cols: u16,
        rows: u16,
        extra_env: &[(String, String)],
    ) -> anyhow::Result<Self> {
        let winsize = Winsize { ws_col: cols, ws_row: rows, ws_xpixel: 0, ws_ypixel: 0 };

        // SAFETY: forkpty is unsafe because the child is in a
        // partially-initialized state after fork. We immediately exec.
        let result = unsafe { forkpty(&winsize, None) }.context("forkpty failed")?;

        match result {
            ForkptyResult::Child => {
                // Child process: restore default signal handlers and exec.
                // Tokio sets SIGPIPE to SIG_IGN which the child inherits;
                // restore it so piped programs behave normally.
                // SAFETY: signal() is unsafe because it changes process-wide
                // signal disposition; in the post-fork child before exec this
                // is the expected place to do so.
                unsafe {
                    let _ = nix::sys::signal::signal(Signal::SIGPIPE, SigHandler::SigDfl);
                }
                std::env::set_var("TERM", "xterm-256color");
                std::env::set_var("COOP", "1");
                for (key, val) in extra_env {
                    std::env::set_var(key, val);
                }

                let c_args: Vec<CString> = command
                    .iter()
                    .map(|s| CString::new(s.as_bytes()))
                    .collect::<Result<_, _>>()
                    .context("invalid command argument")?;

                execvp(&c_args[0], &c_args).context("execvp failed")?;
                unreachable!();
            }
            ForkptyResult::Parent { child, master } => {
                set_nonblocking(&master)?;
                let afd = AsyncFd::new(PtyFd(master)).context("AsyncFd::new failed")?;
                Ok(Self {
                    master: afd,
                    child_pid: child,
                    cols: Arc::new(AtomicU16::new(cols)),
                    rows: Arc::new(AtomicU16::new(rows)),
                    reap_interval: Duration::from_millis(50),
                })
            }
        }
    }

    pub fn with_reap_interval(mut self, interval: Duration) -> Self {
        self.reap_interval = interval;
        self
    }
}

impl Backend for NativePty {
    fn run(
        &mut self,
        output_tx: mpsc::Sender<Bytes>,
        mut input_rx: mpsc::Receiver<BackendInput>,
        mut resize_rx: mpsc::Receiver<(u16, u16)>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<ExitStatus>> + Send + '_>>
    {
        let pid = self.child_pid;
        Box::pin(async move {
            let mut buf = vec![0u8; 8192];
            let mut input_closed = false;

            loop {
                if input_closed {
                    // Read output + handle resize once input is closed
                    tokio::select! {
                        result = read_chunk(&self.master, &mut buf) => {
                            match result {
                                Ok(0) => break,
                                Ok(n) => {
                                    let data = Bytes::copy_from_slice(&buf[..n]);
                                    if output_tx.send(data).await.is_err() {
                                        break;
                                    }
                                }
                                Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                                Err(e) => return Err(e.into()),
                            }
                        }
                        resize = resize_rx.recv() => {
                            if let Some((cols, rows)) = resize {
                                let _ = self.resize(cols, rows);
                            }
                        }
                    }
                } else {
                    tokio::select! {
                        result = read_chunk(&self.master, &mut buf) => {
                            match result {
                                Ok(0) => break,
                                Ok(n) => {
                                    let data = Bytes::copy_from_slice(&buf[..n]);
                                    if output_tx.send(data).await.is_err() {
                                        break;
                                    }
                                }
                                Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                                Err(e) => return Err(e.into()),
                            }
                        }
                        input = input_rx.recv() => {
                            match input {
                                Some(BackendInput::Write(data)) => {
                                    if let Err(e) = write_all(&self.master, &data).await {
                                        if e.raw_os_error() == Some(libc::EIO) {
                                            break; // Child exited; fall through to wait_for_exit
                                        }
                                        return Err(e.into());
                                    }
                                }
                                Some(BackendInput::Drain(tx)) => {
                                    let _ = tx.send(());
                                }
                                None => input_closed = true,
                            }
                        }
                        resize = resize_rx.recv() => {
                            if let Some((cols, rows)) = resize {
                                let _ = self.resize(cols, rows);
                            }
                        }
                    }
                }
            }

            // Reap child on a blocking thread to avoid blocking the runtime
            let status = tokio::task::spawn_blocking(move || wait_for_exit(pid))
                .await
                .context("join wait thread")??;
            Ok(status)
        })
    }

    fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.cols.store(cols, Ordering::Relaxed);
        self.rows.store(rows, Ordering::Relaxed);

        let ws =
            rustix::termios::Winsize { ws_col: cols, ws_row: rows, ws_xpixel: 0, ws_ypixel: 0 };
        rustix::termios::tcsetwinsize(self.master.get_ref(), ws)
            .context("TIOCSWINSZ ioctl failed")?;

        Ok(())
    }

    fn child_pid(&self) -> Option<u32> {
        Some(self.child_pid.as_raw() as u32)
    }
}

impl Drop for NativePty {
    fn drop(&mut self) {
        // forkpty places the child in a new session (setsid), so the child PID
        // equals the process group ID. Signal the entire group to clean up
        // grandchildren as well.
        let pgid = Pid::from_raw(-self.child_pid.as_raw());

        // Best-effort graceful shutdown: SIGHUP to the process group.
        let _ = kill(pgid, Signal::SIGHUP);

        // Poll for exit up to 500ms before escalating to SIGKILL.
        let iterations = (500 / self.reap_interval.as_millis().max(1)) as usize;
        for _ in 0..iterations.max(1) {
            match waitpid(self.child_pid, Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::Exited(..)) | Ok(WaitStatus::Signaled(..)) => return,
                _ => std::thread::sleep(self.reap_interval),
            }
        }

        let _ = kill(pgid, Signal::SIGKILL);
        let _ = waitpid(self.child_pid, Some(WaitPidFlag::WNOHANG));
    }
}

/// Block until the child exits and convert to our `ExitStatus`.
fn wait_for_exit(pid: Pid) -> anyhow::Result<ExitStatus> {
    loop {
        match waitpid(pid, None) {
            Ok(WaitStatus::Exited(_, code)) => {
                return Ok(ExitStatus { code: Some(code), signal: None });
            }
            Ok(WaitStatus::Signaled(_, sig, _)) => {
                return Ok(ExitStatus { code: None, signal: Some(sig as i32) });
            }
            Ok(_) => continue,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => bail!("waitpid failed: {e}"),
        }
    }
}
