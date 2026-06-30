use std::sync::Arc;
use std::time::Duration;

use muxio_core::rpc::rpc_internals::RpcStreamEvent;
use muxio_rpc_service::prebuffered::RpcMethodPrebuffered;
use muxio_rpc_service_endpoint::{RpcServiceEndpointInterface, StreamResponder};
use muxio_tokio_rpc_ipc_server::{
    RpcIpcServer, RpcIpcServerEvent,
};
use portable_pty::PtySize;
use tokio::sync::{Mutex, mpsc};

use term_session_muxio_service_definitions::{
    CloseSession, ListSessions, ResizePty, Spawn, WriteInput, STREAM_INPUT_METHOD_ID,
    SUBSCRIBE_OUTPUT_METHOD_ID,
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
}

struct SubscriberEntry {
    conn_id: usize,
    respond: StreamResponder,
}

struct ServerState {
    session: Option<Session>,
    clients: Vec<ClientEntry>,
    subscribers: Vec<SubscriberEntry>,
}

impl ServerState {
    fn new() -> Self {
        Self {
            session: None,
            clients: Vec::new(),
            subscribers: Vec::new(),
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
        .register_prebuffered(Spawn::METHOD_ID, move |payload, _ctx| {
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

    // Register SubscribeOutput (streaming handler for PTY output pushes)
    let st = Arc::clone(&state);
    endpoint
        .register_stream_handler(
            SUBSCRIBE_OUTPUT_METHOD_ID,
            move |event, respond, ctx| {
                // Store the StreamResponder on the very first event (Header)
                // so the push loop can start sending output immediately.
                let is_new = matches!(&event, RpcStreamEvent::Header { .. });
                if is_new
                    && let Ok(mut guard) = st.try_lock()
                {
                    // Generate snapshot while holding the lock
                    let snapshot = guard.session.as_mut().map(|s| s.generate_snapshot());
                    guard.subscribers.push(SubscriberEntry {
                        conn_id: ctx.conn_id,
                        respond: respond.clone(),
                    });
                    drop(guard);
                    // Send snapshot through the responder (will be buffered
                    // until set_writer is called after read_bytes returns)
                    if let Some(data) = snapshot
                        && !data.is_empty()
                    {
                        respond.respond(data, false);
                    }
                }
            },
        )
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
                    });
                }
                RpcIpcServerEvent::ClientDisconnected(conn_id) => {
                    tracing::info!("Client {conn_id} disconnected");
                    let mut guard = st.lock().await;
                    guard.clients.retain(|c| c.conn_id != conn_id);
                    guard.subscribers.retain(|s| s.conn_id != conn_id);
                }
            }
        }
    });

    // Output polling and push via stored StreamResponders
    let st = Arc::clone(&state);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(8));
        loop {
            interval.tick().await;

            let mut guard = st.lock().await;

            if guard.subscribers.is_empty() {
                if let Some(session) = guard.session.as_mut() {
                    session.read_output();
                }
                continue;
            }

            let Some(session) = guard.session.as_mut() else {
                break;
            };

            let raw = session.read_output();
            let exited = session.check_exited();

            // Push raw PTY output to all subscribers via StreamResponder
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
