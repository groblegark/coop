// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::driver::AgentState;
use bytes::Bytes;
use nix::sys::signal::Signal;

/// Raw or rendered output from the terminal backend.
#[derive(Debug, Clone)]
pub enum OutputEvent {
    Raw(Bytes),
    ScreenUpdate { seq: u64 },
}

/// Agent state transition with sequence number for ordering.
#[derive(Debug, Clone)]
pub struct TransitionEvent {
    pub prev: AgentState,
    pub next: AgentState,
    pub seq: u64,
    pub cause: String,
    /// Snapshot of the last assistant message text at the time of this transition.
    pub last_message: Option<String>,
}

/// Input sent to the child process through the PTY.
#[derive(Debug)]
pub enum InputEvent {
    Write(Bytes),
    Resize {
        cols: u16,
        rows: u16,
    },
    Signal(PtySignal),
    /// Drain marker: notifies the sender (via oneshot) when all prior
    /// writes have been flushed to the PTY.  Used by `deliver_steps` to
    /// ensure the delay timer starts *after* the PTY write completes.
    WaitForDrain(tokio::sync::oneshot::Sender<()>),
}

/// A prompt response was delivered to the agent's terminal (auto-dismiss or API).
#[derive(Debug, Clone)]
pub struct PromptOutcome {
    /// How the response was triggered: `"groom"` (auto-dismiss) or `"api"`.
    pub source: String,
    /// Prompt type responded to (e.g. `"setup"`, `"permission"`).
    pub r#type: String,
    /// Prompt subtype (e.g. `"login_success"`, `"trust"`).
    pub subtype: Option<String>,
    /// Option number selected (e.g. 1 for "Yes"), or `None` for Enter-only.
    pub option: Option<u32>,
}

/// Raw hook event JSON from the hook FIFO pipe.
#[derive(Debug, Clone)]
pub struct RawHookEvent {
    pub json: serde_json::Value,
}

/// Raw agent message JSON from stdout (Tier 3) or log file (Tier 2).
#[derive(Debug, Clone)]
pub struct RawMessageEvent {
    pub json: serde_json::Value,
    /// Origin of the message: `"stdout"` (Tier 3) or `"log"` (Tier 2).
    pub source: String,
}

/// Named signals that can be delivered to the child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtySignal {
    Hup,
    Int,
    Quit,
    Kill,
    Usr1,
    Usr2,
    Term,
    Cont,
    Stop,
    Tstp,
    Winch,
}

impl PtySignal {
    /// Parse a signal name (e.g. "SIGINT", "INT", "2") into a `PtySignal`.
    pub fn from_name(name: &str) -> Option<Self> {
        let upper = name.to_uppercase();
        let bare: &str = match upper.strip_prefix("SIG") {
            Some(s) => s,
            None => &upper,
        };

        match bare {
            "HUP" | "1" => Some(Self::Hup),
            "INT" | "2" => Some(Self::Int),
            "QUIT" | "3" => Some(Self::Quit),
            "KILL" | "9" => Some(Self::Kill),
            "USR1" | "10" => Some(Self::Usr1),
            "USR2" | "12" => Some(Self::Usr2),
            "TERM" | "15" => Some(Self::Term),
            "CONT" | "18" => Some(Self::Cont),
            "STOP" | "19" => Some(Self::Stop),
            "TSTP" | "20" => Some(Self::Tstp),
            "WINCH" | "28" => Some(Self::Winch),
            _ => None,
        }
    }

    /// Convert to the corresponding `nix` signal for delivery.
    pub fn to_nix(self) -> Signal {
        match self {
            Self::Hup => Signal::SIGHUP,
            Self::Int => Signal::SIGINT,
            Self::Quit => Signal::SIGQUIT,
            Self::Kill => Signal::SIGKILL,
            Self::Usr1 => Signal::SIGUSR1,
            Self::Usr2 => Signal::SIGUSR2,
            Self::Term => Signal::SIGTERM,
            Self::Cont => Signal::SIGCONT,
            Self::Stop => Signal::SIGSTOP,
            Self::Tstp => Signal::SIGTSTP,
            Self::Winch => Signal::SIGWINCH,
        }
    }
}
