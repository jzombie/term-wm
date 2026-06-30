use std::sync::Arc;
use std::time::Duration;

use muxio_core::rpc::RpcRequest;
use muxio_core::rpc::rpc_internals::RpcStreamEvent;
use muxio_rpc_service::prebuffered::RpcMethodPrebuffered;
use muxio_rpc_service_caller::RpcServiceCallerInterface;
use muxio_rpc_service_endpoint::RpcServiceEndpointInterface;
use muxio_tokio_rpc_ipc_server::{
    RpcIpcConnectionContextHandle, RpcIpcServer, RpcIpcServerEvent,
};
use portable_pty::PtySize;
use tokio::sync::{Mutex, mpsc};

use term_session_muxio_service_definitions::{
    CloseSession, ListSessions, PushOutput, ResizePty, SessionPushFrame, Spawn, WriteInput,
    STREAM_INPUT_METHOD_ID,
};

use crate::session::Session;

pub struct SessionServerConfig {
    pub socket_path: String,
    pub cmd: Vec<String>,
    pub cols: u16,
    pub rows: u16,
}

struct ClientEntry {
    conn_id: usize,
    handle: RpcIpcConnectionContextHandle,
}

struct ServerState {
    session: Option<Session>,
    clients: Vec<ClientEntry>,
}

impl ServerState {
    fn new() -> Self {
        Self {
            session: None,
            clients: Vec::new(),
        }
    }
}

type SharedState = Arc<Mutex<ServerState>>;

pub async fn run_server(
    config: SessionServerConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state: SharedState = Arc::new(Mutex::new(ServerState::new()));

    // Spawn initial session
    {
        let mut st = state.lock().await;
        let cmd = if config.cmd.is_empty() {
            None
        } else {
            Some(config.cmd.clone())
        };
        let session = Session::spawn(1, cmd, config.cols, config.rows)?;
        st.session = Some(session);
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

                // If a session already exists and hasn't exited, resize and return it.
                if let Some(ref mut session) = guard.session
                    && !session.exited
                {
                    let size = PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    };
                    let _ = session.pty.resize(size);
                    session.parser.screen_mut().set_size(rows, cols);
                    session.cols = cols;
                    session.rows = rows;

                    // Push a screen snapshot to the calling client so its parser
                    // gets the current state at the correct terminal size.
                    let snapshot = session.generate_snapshot();
                    if !snapshot.is_empty() {
                        let frame = SessionPushFrame::RawOutput {
                            id: session.id,
                            data: snapshot,
                        }
                        .encode();
                        let request = RpcRequest {
                            rpc_method_id: PushOutput::METHOD_ID,
                            rpc_param_bytes: None,
                            rpc_prebuffered_payload_bytes: Some(frame),
                            is_finalized: true,
                        };
                        let handle = RpcIpcConnectionContextHandle(ctx);
                        tokio::spawn(async move {
                            let _ = handle
                                .call_rpc_buffered::<(), _>(request, |_bytes| ())
                                .await;
                        });
                    }

                    return Spawn::encode_response(session.id)
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
                }

                let id = 1;
                let session = Session::spawn(id, cmd, cols, rows)?;
                guard.session = Some(session);
                Spawn::encode_response(id)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            }
        })
        .await
        .map_err(|e| format!("register Spawn: {e:?}"))?;

    // Register ResizePty
    let st = Arc::clone(&state);
    endpoint
        .register_prebuffered(ResizePty::METHOD_ID, move |payload, _ctx| {
            let state = Arc::clone(&st);
            async move {
                let (_id, cols, rows) = ResizePty::decode_request(&payload)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                let mut guard = state.lock().await;
                if let Some(session) = guard.session.as_mut() {
                    let size = portable_pty::PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    };
                    let _ = session.pty.resize(size);
                    session.parser.screen_mut().set_size(rows, cols);
                }
                ResizePty::encode_response(())
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
                if let Some(session) = guard.session.as_mut() {
                    let _ = session.pty.kill_child();
                }
                guard.session = None;
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
                if let Some(session) = guard.session.as_mut() && session.id == id {
                    let _ = session.pty.write_bytes(&data);
                }
                WriteInput::encode_response(())
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            }
        })
        .await
        .map_err(|e| format!("register WriteInput: {e:?}"))?;

    // Register StreamInput (streaming handler for PTY input)
    let st = Arc::clone(&state);
    endpoint
        .register_stream_handler(STREAM_INPUT_METHOD_ID, move |event, _emit, _ctx| {
            if let RpcStreamEvent::PayloadChunk { bytes, .. } = event
                && let Ok(mut guard) = st.try_lock()
                && let Some(session) = guard.session.as_mut()
            {
                let _ = session.pty.write_bytes(&bytes);
            }
        })
        .await
        .map_err(|e| format!("register stream handler STREAM_INPUT: {e:?}"))?;

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
                        handle,
                    });
                }
                RpcIpcServerEvent::ClientDisconnected(conn_id) => {
                    tracing::info!("Client {conn_id} disconnected");
                    let mut guard = st.lock().await;
                    guard.clients.retain(|c| c.conn_id != conn_id);
                }
            }
        }
    });

    // Output polling and push loop
    let st = Arc::clone(&state);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(8));
        loop {
            interval.tick().await;

            let (frames, clients) = {
                let mut guard = st.lock().await;

                if guard.clients.is_empty() {
                    if let Some(session) = guard.session.as_mut() {
                        session.read_output();
                    }
                    continue;
                }

                let Some(session) = guard.session.as_mut() else {
                    break;
                };

                let mut frames = Vec::new();

                let raw = session.read_output();
                if !raw.is_empty() {
                    frames.push(
                        SessionPushFrame::RawOutput {
                            id: session.id,
                            data: raw,
                        }
                        .encode(),
                    );
                }

                if let Some(title) = session.title.take()
                    && !title.is_empty()
                {
                    frames.push(
                        SessionPushFrame::TitleChanged {
                            id: session.id,
                            title,
                        }
                        .encode(),
                    );
                }

                if session.check_exited() {
                    frames.push(
                        SessionPushFrame::SessionExited {
                            id: session.id,
                            status: 0,
                        }
                        .encode(),
                    );
                }

                let clients: Vec<RpcIpcConnectionContextHandle> =
                    guard.clients.iter().map(|c| c.handle.clone()).collect();

                (frames, clients)
            };

            // Push frames to all connected clients
            for ctx in &clients {
                for frame in &frames {
                    let request = RpcRequest {
                        rpc_method_id: PushOutput::METHOD_ID,
                        rpc_param_bytes: None,
                        rpc_prebuffered_payload_bytes: Some(frame.to_vec()),
                        is_finalized: true,
                    };
                    match ctx.call_rpc_buffered::<(), _>(request, |_bytes| ()).await {
                        Ok((_encoder, Ok(()))) => {}
                        Ok((_encoder, Err(rpc_err))) => {
                            tracing::warn!("Push response error from client {}: {rpc_err:?}", ctx.0.conn_id);
                        }
                        Err(rpc_err) => {
                            tracing::warn!("Failed to push to client {}: {rpc_err:?}", ctx.0.conn_id);
                        }
                    }
                }
            }

            // Stop push loop if session exited
            let exited = {
                let guard = st.lock().await;
                guard.session.as_ref().is_none_or(|s| s.exited)
            };
            if exited {
                tracing::info!("Session exited; stopping push loop");
                break;
            }
        }
    });

    tracing::info!("Session server listening on {}", config.socket_path);
    server
        .serve(&config.socket_path)
        .await
        .map_err(|e| format!("serve: {e:?}"))?;

    Ok(())
}
