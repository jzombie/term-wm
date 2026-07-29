use std::sync::Arc;

use muxio_core::rpc::rpc_internals::RpcStreamEvent;
use muxio_rpc_service::prebuffered::RpcMethodPrebuffered;
use muxio_rpc_service_endpoint::{RpcServiceEndpointInterface, StreamResponder};
use muxio_tokio_rpc_ipc_server::{RpcIpcServer, RpcIpcServerEvent};
use portable_pty::PtySize;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};

use term_session_muxio_service_definitions::{
    CloseSession, ListSessions, ResizePty, STREAM_INPUT_METHOD_ID, SUBSCRIBE_OUTPUT_METHOD_ID,
    Spawn, WriteInput,
};
use term_wm_pty_engine::PtyStatus;

use crate::session::Session;

pub struct SessionServerConfig {
    pub socket_path: String,
    pub cmd: Vec<String>,
    pub cols: u16,
    pub rows: u16,
}

struct ClientEntry {
    conn_id: usize,
    cols: u16,
    rows: u16,
}

struct SubscriberEntry {
    conn_id: usize,
    respond: StreamResponder,
}

struct ServerState {
    session: Option<Session>,
    clients: Vec<ClientEntry>,
    subscribers: Vec<SubscriberEntry>,
    notify: Arc<Notify>,
}

impl ServerState {
    fn new(notify: Arc<Notify>) -> Self {
        Self {
            session: None,
            clients: Vec::new(),
            subscribers: Vec::new(),
            notify,
        }
    }

    /// Replace the current session and attach the Notify callback
    /// so the background polling task is woken on PTY output.
    fn set_session(&mut self, mut session: Session) {
        let n = self.notify.clone();
        session.set_status_callback(Some(Box::new(move |status| {
            if matches!(status, PtyStatus::Wakeup | PtyStatus::Exited) {
                n.notify_one();
            }
        })));
        self.session = Some(session);
        // Prime notify to process initial startup output generated
        // before the callback was registered.
        self.notify.notify_one();
    }

    /// Terminate and clear the active session, flushing remaining PTY buffers
    /// and stream completion markers to all active subscribers.
    fn clear_session(&mut self) {
        if let Some(mut session) = self.session.take() {
            let _ = session.pty.kill_child();
            let raw = session.read_output();
            if !raw.is_empty() {
                for sub in &self.subscribers {
                    sub.respond.respond(raw.clone(), false);
                }
            }
        }
        for sub in &self.subscribers {
            sub.respond.respond(Vec::new(), true);
        }
        self.subscribers.clear();
        self.notify.notify_one();
    }

    /// Constrain the PTY to the smallest geometry across all connected clients.
    /// This guarantees the virtual buffer never exceeds any attached monitor.
    fn recalculate_pty_size(&mut self) {
        let Some(session) = self.session.as_mut() else { return; };
        if self.clients.is_empty() { return; }

        let min_cols = self.clients.iter()
            .map(|c| c.cols)
            .filter(|&c| c != u16::MAX)
            .min()
            .unwrap_or(80);
        let min_rows = self.clients.iter()
            .map(|c| c.rows)
            .filter(|&r| r != u16::MAX)
            .min()
            .unwrap_or(24);

        let size = PtySize {
            rows: min_rows,
            cols: min_cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let _ = session.pty.resize(size);
        session.cols = min_cols;
        session.rows = min_rows;
    }
}

type SharedState = Arc<Mutex<ServerState>>;

/// Run the session server. Returns the PTY child's exit code on success.
pub async fn run_server(
    config: SessionServerConfig,
) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
    let notify = Arc::new(Notify::new());
    let state: SharedState = Arc::new(Mutex::new(ServerState::new(notify.clone())));

    // Spawn initial session
    {
        let mut st = state.lock().await;
        let cmd = if config.cmd.is_empty() {
            None
        } else {
            Some(config.cmd.clone())
        };
        let session = Session::spawn(1, cmd, config.cols, config.rows)?;
        st.set_session(session);
    }

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let server = RpcIpcServer::new(Some(event_tx));
    let endpoint = server.endpoint();

    // Register Spawn
    let st = Arc::clone(&state);
    endpoint
        .register_prebuffered(Spawn::METHOD_ID, move |payload, ctx| {
            let state = Arc::clone(&st);
            async move {
                let mut guard = state.lock().await;

                let (cmd, cols, rows) = Spawn::decode_request(&payload)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

                // Update calling client's geometry
                if let Some(client) = guard.clients.iter_mut().find(|c| c.conn_id == ctx.conn_id) {
                    client.cols = cols;
                    client.rows = rows;
                }

                // If a session already exists and hasn't exited, reuse it.
                if guard.session.as_ref().is_some_and(|s| !s.exited) {
                    guard.recalculate_pty_size();
                    let id = guard.session.as_ref().map(|s| s.id).unwrap_or(1);
                    let cols = guard.session.as_ref().map(|s| s.cols).unwrap_or(cols);
                    let rows = guard.session.as_ref().map(|s| s.rows).unwrap_or(rows);

                    return Spawn::encode_response((id, cols, rows))
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
                }

                let id = 1;
                let session = Session::spawn(id, cmd, cols, rows)?;
                guard.set_session(session);
                // Enforce global geometric constraints on the newly instantiated PTY
                guard.recalculate_pty_size();

                let session = guard.session.as_ref().unwrap();
                Spawn::encode_response((session.id, session.cols, session.rows))
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            }
        })
        .await
        .map_err(|e| format!("register Spawn: {e:?}"))?;

    // Register ResizePty
    let st = Arc::clone(&state);
    endpoint
        .register_prebuffered(ResizePty::METHOD_ID, move |payload, ctx| {
            let state = Arc::clone(&st);
            async move {
                let (_id, cols, rows) = ResizePty::decode_request(&payload)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                let mut guard = state.lock().await;

                // Update calling client's geometry
                if let Some(client) = guard.clients.iter_mut().find(|c| c.conn_id == ctx.conn_id) {
                    client.cols = cols;
                    client.rows = rows;
                }

                guard.recalculate_pty_size();

                let (actual_cols, actual_rows) = guard.session.as_ref()
                    .map(|s| (s.cols, s.rows))
                    .unwrap_or((cols, rows));

                ResizePty::encode_response((actual_cols, actual_rows))
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            }
        })
        .await
        .map_err(|e| format!("register ResizePty: {e:?}"))?;

    // Register CloseSession
    let st = Arc::clone(&state);
    endpoint
        .register_prebuffered(CloseSession::METHOD_ID, move |payload, _ctx| {
            let state = Arc::clone(&st);
            async move {
                let _id = CloseSession::decode_request(&payload)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                let mut guard = state.lock().await;
                guard.clear_session();
                CloseSession::encode_response(())
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            }
        })
        .await
        .map_err(|e| format!("register CloseSession: {e:?}"))?;

    // Register ListSessions
    let st = Arc::clone(&state);
    endpoint
        .register_prebuffered(ListSessions::METHOD_ID, move |_payload, _ctx| {
            let state = Arc::clone(&st);
            async move {
                let guard = state.lock().await;
                let sessions = match &guard.session {
                    Some(s) => vec![(s.id, String::new(), s.exited)],
                    None => vec![],
                };
                ListSessions::encode_response(sessions)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            }
        })
        .await
        .map_err(|e| format!("register ListSessions: {e:?}"))?;

    // Register WriteInput
    let st = Arc::clone(&state);
    endpoint
        .register_prebuffered(WriteInput::METHOD_ID, move |payload, _ctx| {
            let state = Arc::clone(&st);
            async move {
                let (id, data) = WriteInput::decode_request(&payload)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                let mut guard = state.lock().await;
                if let Some(session) = guard.session.as_mut()
                    && session.id == id
                {
                    let _ = session.pty.write_bytes(&data);
                }
                WriteInput::encode_response(())
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            }
        })
        .await
        .map_err(|e| format!("register WriteInput: {e:?}"))?;

    // Register StreamInput (streaming handler for PTY input)
    // The channel persists across client disconnects so reconnecting
    // clients can still send input — we drop it only when the server
    // shuts down.
    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    endpoint
        .register_stream_handler(STREAM_INPUT_METHOD_ID, move |event, _responder, _ctx| {
            if let RpcStreamEvent::PayloadChunk { bytes, .. } = event {
                let _ = input_tx.send(bytes);
            }
            // Intentionally ignore End/Error — the channel stays alive.
        })
        .await
        .map_err(|e| format!("register stream handler STREAM_INPUT: {e:?}"))?;

    // Background task: write received input bytes to the PTY session
    let input_st = Arc::clone(&state);
    tokio::spawn(async move {
        while let Some(data) = input_rx.recv().await {
            let mut guard = input_st.lock().await;
            if let Some(session) = guard.session.as_mut() {
                let _ = session.pty.write_bytes(&data);
            }
        }
    });

    // Register SubscribeOutput (streaming handler for PTY output pushes)
    let st = Arc::clone(&state);
    endpoint
        .register_stream_handler(SUBSCRIBE_OUTPUT_METHOD_ID, move |event, respond, ctx| {
            let is_new = matches!(&event, RpcStreamEvent::Header { .. });
            if is_new {
                let st = Arc::clone(&st);
                tokio::spawn(async move {
                    let mut guard = st.lock().await;

                    // Drain accumulated PTY output and capture the raw bytes
                    // so they can be sent to the new subscriber (not just the snapshot).
                    let early = guard.session.as_mut().and_then(|s| {
                        let data = s.read_output();
                        if data.is_empty() { None } else { Some(data) }
                    });
                    let snapshot = guard.session.as_mut().map(|s| s.generate_snapshot());

                    guard.subscribers.push(SubscriberEntry {
                        conn_id: ctx.conn_id,
                        respond: respond.clone(),
                    });

                    // Wake the polling loop — the session may have pending
                    // output or exit state that needs processing.
                    guard.notify.notify_one();

                    let is_dead = guard.session.is_none();
                    drop(guard);

                    if let Some(data) = snapshot
                        && !data.is_empty()
                    {
                        respond.respond(data, false);
                    }
                    if let Some(data) = early {
                        respond.respond(data, false);
                    }
                    if is_dead {
                        respond.respond(Vec::new(), true);
                    }
                });
            }
        })
        .await
        .map_err(|e| format!("register SubscribeOutput: {e:?}"))?;

    // Connection event handler
    let st = Arc::clone(&state);
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                RpcIpcServerEvent::ClientConnected(handle) => {
                    tracing::info!("Client {} connected", handle.0.conn_id);

                    let mut guard = st.lock().await;
                    guard.clients.push(ClientEntry {
                        conn_id: handle.0.conn_id,
                        cols: u16::MAX,
                        rows: u16::MAX,
                    });
                }
                RpcIpcServerEvent::ClientDisconnected(conn_id) => {
                    tracing::info!("Client {conn_id} disconnected");
                    let mut guard = st.lock().await;
                    guard.clients.retain(|c| c.conn_id != conn_id);
                    guard.subscribers.retain(|s| s.conn_id != conn_id);
                    guard.recalculate_pty_size();
                }
            }
        }
    });

    // Output polling via Notify — blocks until PTY produces output.
    // When the session exits, the exit code is sent back through this
    // channel so run_server can return it.
    let (exit_tx, mut exit_rx) = oneshot::channel::<i32>();
    let st = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            // Block until PTY actually produces output — 0 wakeups when idle
            notify.notified().await;

            let mut guard = st.lock().await;

            if guard.subscribers.is_empty() {
                let mut exited = false;
                if let Some(session) = guard.session.as_mut() {
                    session.sync_screen();
                    exited = session.check_exited();
                }
                if exited {
                    tracing::info!("Session exited, tearing down");
                    guard.session = None;
                }
                continue;
            }

            let (raw, exited, code) = {
                let Some(session) = guard.session.as_mut() else {
                    let _ = exit_tx.send(0);
                    break;
                };

                let raw = session.read_output();
                let exited = session.check_exited();
                let code = session.exit_code;
                (raw, exited, code)
            };

            if raw.is_empty() && !guard.subscribers.is_empty() {
                tracing::debug!(
                    "PTY output empty with {} subscribers",
                    guard.subscribers.len()
                );
            }

            // Push raw PTY output to all subscribers
            if !raw.is_empty() {
                for sub in &guard.subscribers {
                    sub.respond.respond(raw.clone(), false);
                }
            }

            // On exit: finalize all streams and clean up
            if exited {
                for sub in &guard.subscribers {
                    sub.respond.respond(Vec::new(), true);
                }
                guard.subscribers.clear();
                let _ = exit_tx.send(code.unwrap_or(0));
                tracing::info!("Session exited with code {:?}", code);
                break;
            }
        }
    });

    tracing::info!("Session server listening on {}", config.socket_path);

    // Wait for either the server to finish or the session to exit.
    let exit_code = tokio::select! {
        result = server.serve(&config.socket_path) => {
            result.map_err(|e| format!("serve: {e:?}"))?;
            0
        }
        code = &mut exit_rx => {
            code.unwrap_or(0)
        }
    };

    Ok(exit_code)
}
