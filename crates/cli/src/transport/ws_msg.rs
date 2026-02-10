// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! WebSocket message types and conversions.

use serde::{Deserialize, Serialize};

use crate::driver::{AgentState, PromptContext};
use crate::error::ErrorCode;
use crate::event::StateChangeEvent;
use crate::screen::{CursorPosition, ScreenSnapshot};
use crate::start::StartEvent;
use crate::stop::StopEvent;
use crate::transport::handler::{
    extract_error_fields, NudgeOutcome, RespondOutcome, SessionStatus,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Output {
        data: String,
        offset: u64,
    },
    Screen {
        lines: Vec<String>,
        cols: u16,
        rows: u16,
        alt_screen: bool,
        cursor: Option<CursorPosition>,
        seq: u64,
    },
    StateChange {
        prev: String,
        next: String,
        seq: u64,
        prompt: Box<Option<PromptContext>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_detail: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_category: Option<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        cause: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        last_message: Option<String>,
    },
    Exit {
        code: Option<i32>,
        signal: Option<i32>,
    },
    Error {
        code: String,
        message: String,
    },
    NudgeResult {
        delivered: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        state_before: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    RespondResult {
        delivered: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Status {
        state: String,
        pid: Option<i32>,
        uptime_secs: i64,
        exit_code: Option<i32>,
        screen_seq: u64,
        bytes_read: u64,
        bytes_written: u64,
        ws_clients: i32,
    },
    Stop {
        stop_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signal: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_detail: Option<String>,
        seq: u64,
    },
    Start {
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        injected: bool,
        seq: u64,
    },
    PromptAction {
        source: String,
        prompt_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        subtype: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        option: Option<u32>,
    },
    InputResult {
        bytes_written: i32,
    },
    ResizeResult {
        cols: u16,
        rows: u16,
    },
    SignalResult {
        delivered: bool,
    },
    ShutdownResult {
        accepted: bool,
    },
    AgentState {
        agent: String,
        state: String,
        since_seq: u64,
        screen_seq: u64,
        detection_tier: String,
        detection_cause: String,
        prompt: Option<PromptContext>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_detail: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_category: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        last_message: Option<String>,
    },
    Health {
        status: String,
        pid: Option<i32>,
        uptime_secs: i64,
        agent: String,
        terminal_cols: u16,
        terminal_rows: u16,
        ws_clients: i32,
        ready: bool,
    },
    Ready {
        ready: bool,
    },
    StopConfig {
        config: serde_json::Value,
    },
    StartConfig {
        config: serde_json::Value,
    },
    ConfigUpdated {
        updated: bool,
    },
    ResolveStopResult {
        accepted: bool,
    },
    ReplayResult {
        data: String,
        offset: u64,
        next_offset: u64,
        total_written: u64,
    },
    Pong {},
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Input {
        text: String,
        #[serde(default)]
        enter: bool,
    },
    InputRaw {
        data: String,
    },
    Keys {
        keys: Vec<String>,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    ScreenRequest {
        #[serde(default)]
        cursor: bool,
    },
    StateRequest {},
    StatusRequest {},
    Nudge {
        message: String,
    },
    Respond {
        accept: Option<bool>,
        text: Option<String>,
        #[serde(default)]
        answers: Vec<crate::transport::handler::TransportQuestionAnswer>,
        option: Option<i32>,
    },
    Replay {
        offset: u64,
        #[serde(default)]
        limit: Option<usize>,
    },
    Auth {
        token: String,
    },
    Signal {
        signal: String,
    },
    Shutdown {},
    HealthRequest {},
    ReadyRequest {},
    GetStopConfig {},
    PutStopConfig {
        config: serde_json::Value,
    },
    GetStartConfig {},
    PutStartConfig {
        config: serde_json::Value,
    },
    ResolveStop {
        body: serde_json::Value,
    },
    Ping {},
}

/// WebSocket subscription mode (query parameter on upgrade).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubscriptionMode {
    Raw,
    Screen,
    State,
    #[default]
    All,
}

/// Query parameters for WebSocket upgrade.
#[derive(Debug, Clone, Deserialize)]
pub struct WsQuery {
    #[serde(default)]
    pub mode: SubscriptionMode,
    pub token: Option<String>,
}

impl From<SessionStatus> for ServerMessage {
    fn from(st: SessionStatus) -> Self {
        ServerMessage::Status {
            state: st.state,
            pid: st.pid,
            uptime_secs: st.uptime_secs,
            exit_code: st.exit_code,
            screen_seq: st.screen_seq,
            bytes_read: st.bytes_read,
            bytes_written: st.bytes_written,
            ws_clients: st.ws_clients,
        }
    }
}

impl From<NudgeOutcome> for ServerMessage {
    fn from(o: NudgeOutcome) -> Self {
        ServerMessage::NudgeResult {
            delivered: o.delivered,
            state_before: o.state_before,
            reason: o.reason,
        }
    }
}

impl From<RespondOutcome> for ServerMessage {
    fn from(o: RespondOutcome) -> Self {
        ServerMessage::RespondResult {
            delivered: o.delivered,
            prompt_type: o.prompt_type,
            reason: o.reason,
        }
    }
}

/// Build a `ServerMessage::Screen` from a screen snapshot.
pub fn snapshot_to_msg(snap: ScreenSnapshot, seq: u64) -> ServerMessage {
    ServerMessage::Screen {
        lines: snap.lines,
        cols: snap.cols,
        rows: snap.rows,
        alt_screen: snap.alt_screen,
        cursor: Some(snap.cursor),
        seq,
    }
}

/// Build a WebSocket error message.
pub fn ws_error(code: ErrorCode, message: &str) -> ServerMessage {
    ServerMessage::Error { code: code.as_str().to_owned(), message: message.to_owned() }
}

/// Convert a `StateChangeEvent` to a `ServerMessage`.
pub fn state_change_to_msg(event: &StateChangeEvent) -> ServerMessage {
    if let AgentState::Exited { status } = &event.next {
        return ServerMessage::Exit { code: status.code, signal: status.signal };
    }
    let (error_detail, error_category) = extract_error_fields(&event.next);
    ServerMessage::StateChange {
        prev: event.prev.as_str().to_owned(),
        next: event.next.as_str().to_owned(),
        seq: event.seq,
        prompt: Box::new(event.next.prompt().cloned()),
        error_detail,
        error_category,
        cause: event.cause.clone(),
        last_message: event.last_message.clone(),
    }
}

/// Convert a `StartEvent` to a `ServerMessage`.
pub fn start_event_to_msg(event: &StartEvent) -> ServerMessage {
    ServerMessage::Start {
        source: event.source.clone(),
        session_id: event.session_id.clone(),
        injected: event.injected,
        seq: event.seq,
    }
}

/// Convert a `StopEvent` to a `ServerMessage`.
pub fn stop_event_to_msg(event: &StopEvent) -> ServerMessage {
    ServerMessage::Stop {
        stop_type: event.stop_type.as_str().to_owned(),
        signal: event.signal.clone(),
        error_detail: event.error_detail.clone(),
        seq: event.seq,
    }
}
