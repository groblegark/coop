// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! gRPC transport implementing the `Coop` service defined in `coop.v1`.

use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use super::{
    deliver_steps, encode_response, keys_to_bytes, read_ring_combined, update_question_current,
};
use crate::driver::{classify_error_detail, AgentState, PromptContext, QuestionAnswer};
use crate::error::ErrorCode;
use crate::event::{InputEvent, OutputEvent, PtySignal, StateChangeEvent};
use crate::stop::StopConfig;
use crate::transport::state::AppState;

/// Generated protobuf types for the `coop.v1` package.
pub mod proto {
    tonic::include_proto!("coop.v1");
}

// ---------------------------------------------------------------------------
// Type conversions: domain → proto
// ---------------------------------------------------------------------------

/// Convert a domain [`crate::screen::CursorPosition`] to proto.
pub fn cursor_to_proto(c: &crate::screen::CursorPosition) -> proto::CursorPosition {
    proto::CursorPosition { row: c.row as i32, col: c.col as i32 }
}

/// Convert a domain [`crate::screen::ScreenSnapshot`] to proto [`proto::ScreenSnapshot`].
pub fn screen_snapshot_to_proto(s: &crate::screen::ScreenSnapshot) -> proto::ScreenSnapshot {
    proto::ScreenSnapshot {
        lines: s.lines.clone(),
        cols: s.cols as i32,
        rows: s.rows as i32,
        alt_screen: s.alt_screen,
        cursor: Some(cursor_to_proto(&s.cursor)),
        seq: s.sequence,
    }
}

/// Convert a domain [`crate::screen::ScreenSnapshot`] to a [`proto::GetScreenResponse`],
/// optionally omitting the cursor.
pub fn screen_snapshot_to_response(
    s: &crate::screen::ScreenSnapshot,
    include_cursor: bool,
) -> proto::GetScreenResponse {
    proto::GetScreenResponse {
        lines: s.lines.clone(),
        cols: s.cols as i32,
        rows: s.rows as i32,
        alt_screen: s.alt_screen,
        cursor: if include_cursor { Some(cursor_to_proto(&s.cursor)) } else { None },
        seq: s.sequence,
    }
}

/// Convert a domain [`PromptContext`] to proto.
pub fn prompt_to_proto(p: &PromptContext) -> proto::PromptContext {
    proto::PromptContext {
        r#type: p.kind.as_str().to_owned(),
        tool: p.tool.clone(),
        input_preview: p.input_preview.clone(),
        screen_lines: p.screen_lines.clone(),
        questions: p
            .questions
            .iter()
            .map(|q| proto::QuestionContext {
                question: q.question.clone(),
                options: q.options.clone(),
            })
            .collect(),
        question_current: p.question_current as u32,
        options: p.options.clone(),
        options_fallback: p.options_fallback,
    }
}

/// Convert a domain [`StateChangeEvent`] to proto [`proto::AgentStateEvent`].
pub fn state_change_to_proto(e: &StateChangeEvent) -> proto::AgentStateEvent {
    let (error_detail, error_category) = match &e.next {
        AgentState::Error { detail } => {
            (Some(detail.clone()), Some(classify_error_detail(detail).as_str().to_owned()))
        }
        _ => (None, None),
    };
    let cause = if e.cause.is_empty() { None } else { Some(e.cause.clone()) };
    proto::AgentStateEvent {
        prev: e.prev.as_str().to_owned(),
        next: e.next.as_str().to_owned(),
        seq: e.seq,
        prompt: e.next.prompt().map(prompt_to_proto),
        error_detail,
        error_category,
        cause,
    }
}

// ---------------------------------------------------------------------------
// gRPC service
// ---------------------------------------------------------------------------

/// gRPC implementation of the `coop.v1.Coop` service.
pub struct CoopGrpc {
    state: Arc<AppState>,
}

impl CoopGrpc {
    /// Create a new gRPC service backed by the given shared state.
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Build a [`tonic`] router for this service.
    pub fn into_router(self) -> tonic::transport::server::Router {
        tonic::transport::Server::builder().add_service(proto::coop_server::CoopServer::new(self))
    }
}

type GrpcStream<T> = Pin<Box<dyn tokio_stream::Stream<Item = Result<T, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl proto::coop_server::Coop for CoopGrpc {
    // -----------------------------------------------------------------------
    // 1. GetHealth
    // -----------------------------------------------------------------------
    async fn get_health(
        &self,
        _request: Request<proto::GetHealthRequest>,
    ) -> Result<Response<proto::GetHealthResponse>, Status> {
        let pid = self.state.terminal.child_pid.load(Ordering::Relaxed);
        let uptime = self.state.config.started_at.elapsed().as_secs() as i64;
        let ws = self.state.lifecycle.ws_client_count.load(Ordering::Relaxed);

        Ok(Response::new(proto::GetHealthResponse {
            status: "running".to_owned(),
            pid: if pid == 0 { None } else { Some(pid as i32) },
            uptime_secs: uptime,
            agent: self.state.config.agent.to_string(),
            ws_clients: ws,
        }))
    }

    // -----------------------------------------------------------------------
    // 2. GetScreen
    // -----------------------------------------------------------------------
    async fn get_screen(
        &self,
        request: Request<proto::GetScreenRequest>,
    ) -> Result<Response<proto::GetScreenResponse>, Status> {
        let req = request.into_inner();
        let screen = self.state.terminal.screen.read().await;
        let snap = screen.snapshot();
        Ok(Response::new(screen_snapshot_to_response(&snap, req.include_cursor)))
    }

    // -----------------------------------------------------------------------
    // 3. GetStatus
    // -----------------------------------------------------------------------
    async fn get_status(
        &self,
        _request: Request<proto::GetStatusRequest>,
    ) -> Result<Response<proto::GetStatusResponse>, Status> {
        let pid = self.state.terminal.child_pid.load(Ordering::Relaxed);
        let agent = self.state.driver.agent_state.read().await;
        let screen = self.state.terminal.screen.read().await;
        let ring = self.state.terminal.ring.read().await;
        let uptime = self.state.config.started_at.elapsed().as_secs() as i64;
        let exit = self.state.terminal.exit_status.read().await;

        let exit_code = exit.as_ref().and_then(|e| e.code);

        let bw = self.state.lifecycle.bytes_written.load(Ordering::Relaxed);
        let ws = self.state.lifecycle.ws_client_count.load(Ordering::Relaxed);

        let state_str = match &*agent {
            AgentState::Exited { .. } => "exited",
            _ => {
                if pid == 0 {
                    "starting"
                } else {
                    "running"
                }
            }
        };

        Ok(Response::new(proto::GetStatusResponse {
            state: state_str.to_owned(),
            pid: if pid == 0 { None } else { Some(pid as i32) },
            uptime_secs: uptime,
            exit_code,
            screen_seq: screen.seq(),
            bytes_read: ring.total_written(),
            bytes_written: bw,
            ws_clients: ws,
        }))
    }

    // -----------------------------------------------------------------------
    // 4. SendInput
    // -----------------------------------------------------------------------
    async fn send_input(
        &self,
        request: Request<proto::SendInputRequest>,
    ) -> Result<Response<proto::SendInputResponse>, Status> {
        let req = request.into_inner();
        let mut payload = req.text.into_bytes();
        if req.enter {
            payload.push(b'\r');
        }
        let len = payload.len() as i32;
        self.state
            .channels
            .input_tx
            .send(InputEvent::Write(Bytes::from(payload)))
            .await
            .map_err(|_| ErrorCode::Internal.to_grpc_status("input channel closed"))?;
        Ok(Response::new(proto::SendInputResponse { bytes_written: len }))
    }

    // -----------------------------------------------------------------------
    // 5. SendKeys
    // -----------------------------------------------------------------------
    async fn send_keys(
        &self,
        request: Request<proto::SendKeysRequest>,
    ) -> Result<Response<proto::SendKeysResponse>, Status> {
        let req = request.into_inner();
        let data = keys_to_bytes(&req.keys).map_err(|bad_key| {
            ErrorCode::BadRequest.to_grpc_status(format!("unknown key: {bad_key}"))
        })?;
        let len = data.len() as i32;
        self.state
            .channels
            .input_tx
            .send(InputEvent::Write(Bytes::from(data)))
            .await
            .map_err(|_| ErrorCode::Internal.to_grpc_status("input channel closed"))?;
        Ok(Response::new(proto::SendKeysResponse { bytes_written: len }))
    }

    // -----------------------------------------------------------------------
    // 6. Resize
    // -----------------------------------------------------------------------
    async fn resize(
        &self,
        request: Request<proto::ResizeRequest>,
    ) -> Result<Response<proto::ResizeResponse>, Status> {
        let req = request.into_inner();
        if req.cols <= 0 || req.rows <= 0 {
            return Err(ErrorCode::BadRequest.to_grpc_status("cols and rows must be positive"));
        }
        let cols = req.cols as u16;
        let rows = req.rows as u16;
        self.state
            .channels
            .input_tx
            .send(InputEvent::Resize { cols, rows })
            .await
            .map_err(|_| ErrorCode::Internal.to_grpc_status("input channel closed"))?;
        Ok(Response::new(proto::ResizeResponse { cols: cols as i32, rows: rows as i32 }))
    }

    // -----------------------------------------------------------------------
    // 7. SendSignal
    // -----------------------------------------------------------------------
    async fn send_signal(
        &self,
        request: Request<proto::SendSignalRequest>,
    ) -> Result<Response<proto::SendSignalResponse>, Status> {
        let req = request.into_inner();
        let sig = PtySignal::from_name(&req.signal).ok_or_else(|| {
            ErrorCode::BadRequest.to_grpc_status(format!("unknown signal: {}", req.signal))
        })?;
        self.state
            .channels
            .input_tx
            .send(InputEvent::Signal(sig))
            .await
            .map_err(|_| ErrorCode::Internal.to_grpc_status("input channel closed"))?;
        Ok(Response::new(proto::SendSignalResponse { delivered: true }))
    }

    // -----------------------------------------------------------------------
    // 8. GetAgentState
    // -----------------------------------------------------------------------
    async fn get_agent_state(
        &self,
        _request: Request<proto::GetAgentStateRequest>,
    ) -> Result<Response<proto::GetAgentStateResponse>, Status> {
        let agent = self.state.driver.agent_state.read().await;
        let screen = self.state.terminal.screen.read().await;

        Ok(Response::new(proto::GetAgentStateResponse {
            agent: self.state.config.agent.to_string(),
            state: agent.as_str().to_owned(),
            since_seq: self.state.driver.state_seq.load(Ordering::Relaxed),
            screen_seq: screen.seq(),
            detection_tier: self.state.driver.detection_tier_str(),
            prompt: agent.prompt().map(prompt_to_proto),
            error_detail: self.state.driver.error_detail.read().await.clone(),
            error_category: self
                .state
                .driver
                .error_category
                .read()
                .await
                .map(|c| c.as_str().to_owned()),
        }))
    }

    // -----------------------------------------------------------------------
    // 9. Nudge
    // -----------------------------------------------------------------------
    async fn nudge(
        &self,
        request: Request<proto::NudgeRequest>,
    ) -> Result<Response<proto::NudgeResponse>, Status> {
        if !self.state.ready.load(Ordering::Acquire) {
            return Err(ErrorCode::NotReady.to_grpc_status("agent is still starting"));
        }

        let req = request.into_inner();

        let encoder = self
            .state
            .config
            .nudge_encoder
            .as_ref()
            .ok_or_else(|| ErrorCode::NoDriver.to_grpc_status("no nudge encoder configured"))?;

        let _delivery = self.state.nudge_mutex.lock().await;

        let agent = self.state.driver.agent_state.read().await;
        let state_before = agent.as_str().to_owned();

        match &*agent {
            AgentState::WaitingForInput => {}
            other => {
                return Ok(Response::new(proto::NudgeResponse {
                    delivered: false,
                    state_before,
                    reason: Some(format!("agent is {}", other.as_str())),
                }));
            }
        }

        let steps = encoder.encode(&req.message);
        drop(agent);

        deliver_steps(&self.state.channels.input_tx, steps)
            .await
            .map_err(|code| code.to_grpc_status("input channel closed"))?;

        Ok(Response::new(proto::NudgeResponse { delivered: true, state_before, reason: None }))
    }

    // -----------------------------------------------------------------------
    // 10. Respond
    // -----------------------------------------------------------------------
    async fn respond(
        &self,
        request: Request<proto::RespondRequest>,
    ) -> Result<Response<proto::RespondResponse>, Status> {
        if !self.state.ready.load(Ordering::Acquire) {
            return Err(ErrorCode::NotReady.to_grpc_status("agent is still starting"));
        }

        let req = request.into_inner();

        let encoder =
            self.state.config.respond_encoder.as_ref().ok_or_else(|| {
                ErrorCode::NoDriver.to_grpc_status("no respond encoder configured")
            })?;

        let answers: Vec<QuestionAnswer> = req
            .answers
            .iter()
            .map(|a| QuestionAnswer { option: a.option.map(|o| o as u32), text: a.text.clone() })
            .collect();

        let _delivery = self.state.nudge_mutex.lock().await;

        let agent = self.state.driver.agent_state.read().await;

        let option = req.option.map(|o| o as u32);
        let (steps, answers_delivered) = encode_response(
            &agent,
            encoder.as_ref(),
            req.accept,
            option,
            req.text.as_deref(),
            &answers,
        )
        .map_err(|code| {
            code.to_grpc_status(format!("agent is {} (no active prompt)", agent.as_str()))
        })?;

        let prompt_type = agent.prompt().map(|p| p.kind.as_str().to_owned()).unwrap_or_default();
        drop(agent);

        deliver_steps(&self.state.channels.input_tx, steps)
            .await
            .map_err(|code| code.to_grpc_status("input channel closed"))?;

        if answers_delivered > 0 {
            update_question_current(&self.state, answers_delivered).await;
        }

        Ok(Response::new(proto::RespondResponse { delivered: true, prompt_type, reason: None }))
    }

    // -----------------------------------------------------------------------
    // 11. StreamOutput
    // -----------------------------------------------------------------------
    type StreamOutputStream = GrpcStream<proto::OutputChunk>;

    async fn stream_output(
        &self,
        request: Request<proto::StreamOutputRequest>,
    ) -> Result<Response<Self::StreamOutputStream>, Status> {
        let from_offset = request.into_inner().from_offset;
        let (tx, rx) = mpsc::channel(64);

        // Replay buffered data from ring buffer
        {
            let ring = self.state.terminal.ring.read().await;
            let data = read_ring_combined(&ring, from_offset);
            if !data.is_empty() {
                let _ = tx.send(Ok(proto::OutputChunk { data, offset: from_offset })).await;
            }
        }

        // Subscribe to live output
        let mut output_rx = self.state.channels.output_tx.subscribe();
        let terminal = Arc::clone(&self.state.terminal);

        tokio::spawn(async move {
            loop {
                match output_rx.recv().await {
                    Ok(OutputEvent::Raw(data)) => {
                        let r = terminal.ring.read().await;
                        let offset = r.total_written() - data.len() as u64;
                        drop(r);
                        let chunk = proto::OutputChunk { data: data.to_vec(), offset };
                        if tx.send(Ok(chunk)).await.is_err() {
                            break;
                        }
                    }
                    Ok(OutputEvent::ScreenUpdate { .. }) => {
                        // Skip screen-update events in raw output stream
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Skip missed messages
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    // -----------------------------------------------------------------------
    // 12. StreamScreen
    // -----------------------------------------------------------------------
    type StreamScreenStream = GrpcStream<proto::ScreenSnapshot>;

    async fn stream_screen(
        &self,
        _request: Request<proto::StreamScreenRequest>,
    ) -> Result<Response<Self::StreamScreenStream>, Status> {
        let (tx, rx) = mpsc::channel(16);
        let mut output_rx = self.state.channels.output_tx.subscribe();
        let terminal = Arc::clone(&self.state.terminal);

        tokio::spawn(async move {
            loop {
                match output_rx.recv().await {
                    Ok(OutputEvent::ScreenUpdate { .. }) => {
                        let s = terminal.screen.read().await;
                        let snap = s.snapshot();
                        drop(s);
                        let proto_snap = screen_snapshot_to_proto(&snap);
                        if tx.send(Ok(proto_snap)).await.is_err() {
                            break;
                        }
                    }
                    Ok(OutputEvent::Raw(_)) => {
                        // Skip raw events in screen stream
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    // -----------------------------------------------------------------------
    // 13. StreamState
    // -----------------------------------------------------------------------
    type StreamStateStream = GrpcStream<proto::AgentStateEvent>;

    async fn stream_state(
        &self,
        _request: Request<proto::StreamStateRequest>,
    ) -> Result<Response<Self::StreamStateStream>, Status> {
        let (tx, rx) = mpsc::channel(16);
        let mut state_rx = self.state.channels.state_tx.subscribe();

        tokio::spawn(async move {
            loop {
                match state_rx.recv().await {
                    Ok(event) => {
                        let proto_event = state_change_to_proto(&event);
                        if tx.send(Ok(proto_event)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    // -----------------------------------------------------------------------
    // 14. ResolveStop
    // -----------------------------------------------------------------------
    async fn resolve_stop(
        &self,
        request: Request<proto::ResolveStopRequest>,
    ) -> Result<Response<proto::ResolveStopResponse>, Status> {
        let req = request.into_inner();
        let body: serde_json::Value = serde_json::from_str(&req.body_json)
            .map_err(|e| Status::invalid_argument(format!("invalid JSON: {e}")))?;
        let stop = &self.state.stop;
        *stop.signal_body.write().await = Some(body);
        stop.signaled.store(true, std::sync::atomic::Ordering::Release);
        Ok(Response::new(proto::ResolveStopResponse { accepted: true }))
    }

    // -----------------------------------------------------------------------
    // 15. PutStopConfig
    // -----------------------------------------------------------------------
    async fn put_stop_config(
        &self,
        request: Request<proto::PutStopConfigRequest>,
    ) -> Result<Response<proto::PutStopConfigResponse>, Status> {
        let req = request.into_inner();
        let new_config: StopConfig = serde_json::from_str(&req.config_json)
            .map_err(|e| Status::invalid_argument(format!("invalid config JSON: {e}")))?;
        *self.state.stop.config.write().await = new_config;
        Ok(Response::new(proto::PutStopConfigResponse { updated: true }))
    }

    // -----------------------------------------------------------------------
    // 16. GetStopConfig
    // -----------------------------------------------------------------------
    async fn get_stop_config(
        &self,
        _request: Request<proto::GetStopConfigRequest>,
    ) -> Result<Response<proto::GetStopConfigResponse>, Status> {
        let config = self.state.stop.config.read().await;
        let json = serde_json::to_string(&*config)
            .map_err(|e| Status::internal(format!("serialize error: {e}")))?;
        Ok(Response::new(proto::GetStopConfigResponse { config_json: json }))
    }

    // -----------------------------------------------------------------------
    // 17. StreamStopEvents
    // -----------------------------------------------------------------------
    type StreamStopEventsStream = GrpcStream<proto::StopEvent>;

    async fn stream_stop_events(
        &self,
        _request: Request<proto::StreamStopEventsRequest>,
    ) -> Result<Response<Self::StreamStopEventsStream>, Status> {
        let (tx, rx) = mpsc::channel(16);
        let mut stop_rx = self.state.stop.stop_tx.subscribe();

        tokio::spawn(async move {
            loop {
                match stop_rx.recv().await {
                    Ok(event) => {
                        let proto_event = proto::StopEvent {
                            stop_type: event.stop_type.as_str().to_owned(),
                            signal_json: event.signal.map(|v| v.to_string()),
                            error_detail: event.error_detail,
                            seq: event.seq,
                        };
                        if tx.send(Ok(proto_event)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

#[cfg(test)]
#[path = "grpc_tests.rs"]
mod tests;
