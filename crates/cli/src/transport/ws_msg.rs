// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! WebSocket message types and conversions.

use serde::{Deserialize, Serialize};

use crate::driver::{AgentState, PromptContext};
use crate::error::ErrorCode;
use crate::event::TransitionEvent;
use crate::screen::{CursorPosition, ScreenSnapshot};
use crate::start::StartEvent;
use crate::stop::StopEvent;
use crate::transport::handler::{
    extract_error_fields, NudgeOutcome, RespondOutcome, SessionStatus,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ClientMessage {
    // Terminal
    #[serde(rename = "health:get")]
    GetHealth {},
    #[serde(rename = "ready:get")]
    GetReady {},
    #[serde(rename = "screen:get")]
    GetScreen {
        #[serde(default)]
        cursor: bool,
    },
    #[serde(rename = "replay:get")]
    GetReplay {
        offset: u64,
        #[serde(default)]
        limit: Option<usize>,
    },
    #[serde(rename = "status:get")]
    GetStatus {},
    #[serde(rename = "input:send")]
    SendInput {
        text: String,
        #[serde(default)]
        enter: bool,
    },
    #[serde(rename = "input:send:raw")]
    SendInputRaw {
        data: String,
    },
    #[serde(rename = "keys:send")]
    SendKeys {
        keys: Vec<String>,
    },
    #[serde(rename = "signal:send")]
    SendSignal {
        signal: String,
    },
    Resize {
        cols: u16,
        rows: u16,
    },

    // Agent
    #[serde(rename = "agent:get")]
    GetAgent {},
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

    // Stop hook
    #[serde(rename = "stop:config:get")]
    GetStopConfig {},
    #[serde(rename = "stop:config:put")]
    PutStopConfig {
        config: serde_json::Value,
    },
    #[serde(rename = "stop:resolve")]
    ResolveStop {
        body: serde_json::Value,
    },

    // Start hook
    #[serde(rename = "config:start:get")]
    GetStartConfig {},
    #[serde(rename = "config:put:get")]
    PutStartConfig {
        config: serde_json::Value,
    },

    // Transcripts
    #[serde(rename = "transcript:list")]
    ListTranscripts {},
    #[serde(rename = "transcript:get")]
    GetTranscript {
        number: u32,
    },
    #[serde(rename = "transcript:catchup")]
    CatchupTranscripts {
        #[serde(default)]
        since_transcript: u32,
        #[serde(default)]
        since_line: u64,
    },

    // Session switch
    #[serde(rename = "session:switch")]
    SwitchSession {
        #[serde(default)]
        credentials: Option<std::collections::HashMap<String, String>>,
        #[serde(default)]
        force: bool,
    },

    // Lifecycle
    Shutdown {},

    // Connection
    Ping {},
    Auth {
        token: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ServerMessage {
    // Terminal
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
    Screen {
        lines: Vec<String>,
        cols: u16,
        rows: u16,
        alt_screen: bool,
        cursor: Option<CursorPosition>,
        seq: u64,
    },
    Replay {
        data: String,
        offset: u64,
        next_offset: u64,
        total_written: u64,
    },
    Output {
        data: String,
        offset: u64,
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
    #[serde(rename = "input:sent")]
    InputSent {
        bytes_written: i32,
    },
    #[serde(rename = "signal:sent")]
    SignalSent {
        delivered: bool,
    },
    Resized {
        cols: u16,
        rows: u16,
    },

    // Agent
    #[serde(rename = "agent")]
    Agent {
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
    Nudged {
        delivered: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        state_before: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Response {
        delivered: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Transition {
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
    #[serde(rename = "prompt:outcome")]
    PromptOutcome {
        source: String,
        r#type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        subtype: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        option: Option<u32>,
    },

    // Raw streams
    #[serde(rename = "hook:raw")]
    HookRaw {
        data: serde_json::Value,
    },
    #[serde(rename = "message:raw")]
    MessageRaw {
        data: serde_json::Value,
        source: String,
    },

    // Stop hook
    #[serde(rename = "stop:config")]
    StopConfig {
        config: serde_json::Value,
    },
    #[serde(rename = "stop:configured")]
    StopConfigured {
        updated: bool,
    },
    #[serde(rename = "stop:resolved")]
    StopResolved {
        accepted: bool,
    },
    #[serde(rename = "stop:outcome")]
    StopOutcome {
        r#type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signal: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_detail: Option<String>,
        seq: u64,
    },

    // Start hook
    #[serde(rename = "start:config")]
    StartConfig {
        config: serde_json::Value,
    },
    #[serde(rename = "start:configured")]
    StartConfigured {
        updated: bool,
    },

    // Transcripts
    #[serde(rename = "transcript:list")]
    TranscriptList {
        transcripts: Vec<crate::transcript::TranscriptMeta>,
    },
    #[serde(rename = "transcript:content")]
    TranscriptContent {
        number: u32,
        content: String,
    },
    #[serde(rename = "transcript:catchup")]
    TranscriptCatchup {
        transcripts: Vec<crate::transcript::CatchupTranscript>,
        live_lines: Vec<String>,
        current_transcript: u32,
        current_line: u64,
    },
    #[serde(rename = "transcript:saved")]
    TranscriptSaved {
        number: u32,
        timestamp: String,
        line_count: u64,
        seq: u64,
    },

    #[serde(rename = "start:outcome")]
    StartOutcome {
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        injected: bool,
        seq: u64,
    },

    // Session switch
    #[serde(rename = "session:switched")]
    SessionSwitched {
        scheduled: bool,
    },

    // Lifecycle
    Shutdown {
        accepted: bool,
    },

    // Connection
    Pong {},
    Error {
        code: String,
        message: String,
    },
}

/// WebSocket subscription flags parsed from `?subscribe=output,screen,state,hooks,messages`.
///
/// Defaults to no messages.
/// Clients must opt-in with `?subscribe=` param.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubscriptionFlags {
    pub output: bool,
    pub screen: bool,
    pub state: bool,
    pub hooks: bool,
    pub messages: bool,
    pub transcripts: bool,
}

impl SubscriptionFlags {
    /// Parse a comma-separated flags string (e.g. `"output,state,hooks"`).
    /// Unknown flag names are silently ignored.
    pub fn parse(s: &str) -> Self {
        let mut flags = Self::default();
        for token in s.split(',') {
            match token.trim() {
                "output" => flags.output = true,
                "screen" => flags.screen = true,
                "state" => flags.state = true,
                "hooks" => flags.hooks = true,
                "messages" => flags.messages = true,
                "transcripts" => flags.transcripts = true,
                _ => {}
            }
        }
        flags
    }
}

/// Query parameters for WebSocket upgrade.
#[derive(Debug, Clone, Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
    /// Comma-separated subscription flags (e.g. `raw,state,hooks`).
    /// Default (absent) = no subscriptions (request-reply only).
    pub subscribe: Option<String>,
}

impl WsQuery {
    /// Resolve the effective subscription flags from query parameters.
    pub fn flags(&self) -> SubscriptionFlags {
        match self.subscribe {
            Some(ref s) => SubscriptionFlags::parse(s),
            None => SubscriptionFlags::default(),
        }
    }
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
        ServerMessage::Nudged {
            delivered: o.delivered,
            state_before: o.state_before,
            reason: o.reason,
        }
    }
}

impl From<RespondOutcome> for ServerMessage {
    fn from(o: RespondOutcome) -> Self {
        ServerMessage::Response {
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

/// Convert a `TransitionEvent` to a `ServerMessage`.
pub fn transition_to_msg(event: &TransitionEvent) -> ServerMessage {
    if let AgentState::Exited { status } = &event.next {
        return ServerMessage::Exit { code: status.code, signal: status.signal };
    }
    let (error_detail, error_category) = extract_error_fields(&event.next);
    ServerMessage::Transition {
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

/// Convert a `StopEvent` to a `ServerMessage`.
pub fn stop_event_to_msg(event: &StopEvent) -> ServerMessage {
    ServerMessage::StopOutcome {
        r#type: event.r#type.as_str().to_owned(),
        signal: event.signal.clone(),
        error_detail: event.error_detail.clone(),
        seq: event.seq,
    }
}

/// Convert a `TranscriptEvent` to a `ServerMessage`.
pub fn transcript_event_to_msg(event: &crate::transcript::TranscriptEvent) -> ServerMessage {
    ServerMessage::TranscriptSaved {
        number: event.number,
        timestamp: event.timestamp.clone(),
        line_count: event.line_count,
        seq: event.seq,
    }
}

/// Convert a `StartEvent` to a `ServerMessage`.
pub fn start_event_to_msg(event: &StartEvent) -> ServerMessage {
    ServerMessage::StartOutcome {
        source: event.source.clone(),
        session_id: event.session_id.clone(),
        injected: event.injected,
        seq: event.seq,
    }
}
