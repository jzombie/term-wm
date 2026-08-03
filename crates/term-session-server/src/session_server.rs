use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use muxio_core::rpc::rpc_internals::RpcStreamEvent;
use muxio_rpc_service::prebuffered::RpcMethodPrebuffered;
use muxio_rpc_service_caller::prebuffered::RpcCallPrebuffered;
use muxio_rpc_service_endpoint::{RpcServiceEndpointInterface, StreamResponder};
use muxio_tokio_rpc_ipc_server::{RpcIpcConnectionContextHandle, RpcIpcServer, RpcIpcServerEvent};
use portable_pty::PtySize;
use tokio::sync::{Mutex, Notify, RwLock, mpsc, oneshot};

use term_session_muxio_service_definitions::{
    Attach, ChannelInfo, ChannelName, ClientInfo, CloseSession, KillChannel, KillClient,
    ListChannels, ListChannelsResponse, OnPtyResized, RPC_ERROR_LIVE_SESSIONS,
    RPC_ERROR_SHUTTING_DOWN, RPC_ERROR_UNATTACHED, ResizePty, STREAM_INPUT_METHOD_ID,
    SUBSCRIBE_OUTPUT_METHOD_ID, SessionInfo, ShutdownGateway, Spawn, WriteInput,
};
use term_wm_pty_engine::PtyStatus;

use crate::session::Session;

/// Session id per channel (each channel hosts a single PTY at a time).
const SESSION_ID: u64 = 1;
/// Bounded input channel capacity — memory safety against extreme input bursts.
const INPUT_CHANNEL_CAPACITY: usize = 128;

/// Grace period to let the transport flush end-of-stream frames after the
/// session exits, before the gateway process terminates.
const SESSION_EXIT_FLUSH_GRACE: std::time::Duration = std::time::Duration::from_millis(100);

/// How often the output polling task wakes to re-check the session's exit
/// status, as a fallback for a missed or raced PTY EOF notification.
const SESSION_EXIT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Grace between SIGTERM and SIGKILL when terminating a session's process tree.
const SIGKILL_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// SIGTERM for cooperative process-group termination. On non-Unix platforms
/// the value is unused (kill paths fall back to `kill_child`); it is kept a
/// named `const` so the call sites read identically.
#[cfg(unix)]
const SIGTERM: i32 = libc::SIGTERM;
#[cfg(not(unix))]
const SIGTERM: i32 = 15;
/// SIGKILL for straggler escalation after the grace window.
#[cfg(unix)]
const SIGKILL: i32 = libc::SIGKILL;
#[cfg(not(unix))]
#[allow(dead_code)]
const SIGKILL: i32 = 9;

/// Grace before the gateway exits after the last ShutdownGateway response is
/// flushed: the RPC handler returns immediately and a detached task sleeps
/// this long so the muxio transport drains the `()` frame before the socket
/// is torn down.
const SHUTDOWN_FLUSH_GRACE_MS: u64 = 50;

/// A connection's bind state. Identity is server-assigned (`conn_id`); a
/// connection must `Attach` before it may spawn/resize/write.
#[derive(Clone)]
enum ConnState {
    Unattached,
    Attached(ChannelName),
}

#[derive(Clone)]
struct ConnEntry {
    handle: RpcIpcConnectionContextHandle,
    state: ConnState,
    hostname: String,
    connected_at_unix: u64,
    /// Client process OS PID (reported at Attach).
    pid: u64,
    /// OS user running the client process (reported at Attach).
    user: String,
    /// Client binary version (reported at Attach).
    version: String,
    /// Remote peer IP for SSH attaches; `None` for local (reported at Attach).
    ssh_ip: Option<String>,
}

#[derive(Clone)]
struct ClientEntry {
    caller: Option<RpcIpcConnectionContextHandle>,
    hostname: String,
    connected_at_unix: u64,
    pid: u64,
    user: String,
    version: String,
    ssh_ip: Option<String>,
    cols: u16,
    rows: u16,
}

struct SubscriberEntry {
    conn_id: usize,
    respond: StreamResponder,
}

/// Per-channel state. One gateway process hosts many channels; each channel
/// owns its own session, connected clients, subscribers, and input channel.
struct ChannelState {
    session: Option<Session>,
    clients: HashMap<usize, ClientEntry>,
    subscribers: Vec<SubscriberEntry>,
    notify: Arc<Notify>,
    /// Unix seconds when the channel was first created on the gateway.
    created_at_unix: u64,
    /// Monotonic creation sequence (across the whole gateway process). Sort key
    /// for `ListChannels`: creation order, newest last. `created_at_unix` is
    /// only second-resolution wall clock and cannot disambiguate same-second
    /// creations, so this monotonic counter is authoritative.
    created_seq: u64,
    /// Command template used to respawn the session after it exits.
    cmd: Vec<String>,
    input_tx: mpsc::Sender<Vec<u8>>,
    /// True between a SIGTERM request and the process group's actual exit (or
    /// the SIGKILL escalation). Cleared when the session is observed exited.
    kill_pending: bool,
    /// Tombstone set by GC just before removal; any Attach that read a stale
    /// Arc re-checks this under the write lock (safe double-checked locking).
    is_reaped: bool,
}

/// Gateway coordination. Two tiers:
/// - `conns` is a read-mostly routing table (RwLock).
/// - `channels` maps channel names to independently-mutexed channel states.
///
/// The gateway never holds more than one lock at a time (resolve-and-drop).
struct ServerState {
    conns: RwLock<HashMap<usize, ConnEntry>>,
    channels: RwLock<HashMap<ChannelName, Arc<Mutex<ChannelState>>>>,
    is_shutting_down: AtomicBool,
    /// Monotonic source of `ChannelState::created_seq` (creation order).
    next_channel_seq: AtomicU64,
}

type SharedState = Arc<ServerState>;

fn rpc_err(message: &str) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(std::io::Error::other(message.to_string()))
}

/// Lift an `io::Error` into the boxed handler error type.
fn boxed_io(e: std::io::Error) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(e)
}

/// Unix seconds for a connection's `connected_at_unix` wire field.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl ChannelState {
    fn new(
        cmd: Vec<String>,
        input_tx: mpsc::Sender<Vec<u8>>,
        notify: Arc<Notify>,
        created_seq: u64,
    ) -> Self {
        Self {
            session: None,
            clients: HashMap::new(),
            subscribers: Vec::new(),
            notify,
            created_at_unix: now_unix(),
            created_seq,
            cmd,
            input_tx,
            kill_pending: false,
            is_reaped: false,
        }
    }

    /// Replace the current session and attach the Notify callback so the
    /// background polling task wakes on PTY output.
    fn set_session(&mut self, mut session: Session) {
        let n = self.notify.clone();
        session.set_status_callback(Some(Box::new(move |status| {
            if matches!(status, PtyStatus::Wakeup | PtyStatus::Exited) {
                n.notify_one();
            }
        })));
        self.session = Some(session);
        // Prime notify to process initial startup output generated before the
        // callback was registered.
        self.notify.notify_one();
    }

    /// Signal the session's process group (non-blocking) and arm the kill
    /// escalation flag. The caller is responsible for spawning the detached
    /// escalation task (see `spawn_kill_escalation`). Mechanism only — no
    /// sleeps, no waits, no `SIGKILL` here.
    fn request_session_kill(&mut self, signal: i32) {
        let _ = &signal;
        if let Some(session) = self.session.as_mut() {
            #[cfg(unix)]
            let _ = session.pty.signal_process_group(signal);
            #[cfg(not(unix))]
            let _ = session.pty.kill_child();
        }
        self.kill_pending = true;
        self.notify.notify_one();
    }

    /// Flush remaining PTY buffers and stream completion markers to all active
    /// subscribers, then drop them. Used by kill paths and on session exit.
    fn finalize_subscribers(&mut self) {
        if let Some(session) = self.session.as_mut() {
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
    }

    /// Constrain the PTY to the smallest geometry across all connected clients.
    /// This guarantees the virtual buffer never exceeds any attached monitor.
    ///
    /// Geometry is strictly client-driven: if no connected client has reported
    /// real dimensions yet (all are still `u16::MAX`), nothing is constrained
    /// and the session keeps its spawn-time size. No hardcoded default size is
    /// ever imposed.
    fn recalculate_pty_size(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(min_cols) = self
            .clients
            .values()
            .map(|c| c.cols)
            .filter(|&c| c != u16::MAX)
            .min()
        else {
            return;
        };
        let Some(min_rows) = self
            .clients
            .values()
            .map(|c| c.rows)
            .filter(|&r| r != u16::MAX)
            .min()
        else {
            return;
        };
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

    /// Broadcast geometry to all connected clients via detached async tasks.
    /// Call AFTER releasing the channel lock.
    fn notify_clients(&self, clients: &[ClientEntry], cols: u16, rows: u16) {
        for client in clients {
            let Some(caller) = client.caller.clone() else {
                continue;
            };
            tokio::spawn(async move {
                if let Err(e) = OnPtyResized::call(&caller, (cols, rows)).await {
                    tracing::debug!(error = ?e, "Failed to deliver OnPtyResized notification");
                }
            });
        }
    }

    fn to_info(&self, name: &ChannelName) -> ChannelInfo {
        let session = self.session.as_ref().map(|s| SessionInfo {
            id: s.id,
            cols: s.cols,
            rows: s.rows,
            exited: s.exited,
            exit_code: s.exit_code,
            title: s.title.clone().unwrap_or_default(),
        });
        // Sort by `conn_id`, which muxio assigns monotonically at connection
        // accept — ascending order = connection order, newest client last.
        let mut clients: Vec<ClientInfo> = self
            .clients
            .iter()
            .map(|(conn_id, c)| ClientInfo {
                conn_id: *conn_id,
                pid: c.pid,
                hostname: c.hostname.clone(),
                connected_at_unix: c.connected_at_unix,
                cols: c.cols,
                rows: c.rows,
                user: c.user.clone(),
                version: c.version.clone(),
                ssh_ip: c.ssh_ip.clone(),
            })
            .collect();
        clients.sort_by_key(|c| c.conn_id);
        ChannelInfo {
            name: name.to_string(),
            created_at_unix: self.created_at_unix,
            session,
            clients,
        }
    }
}

/// Resolve the channel a connection is bound to, or `None` if unattached.
async fn bound_channel(state: &ServerState, conn_id: usize) -> Option<ChannelName> {
    let conns = state.conns.read().await;
    match conns.get(&conn_id)?.state {
        ConnState::Attached(ref name) => Some(name.clone()),
        ConnState::Unattached => None,
    }
}

/// Fetch an `Arc<ChannelState>` for the channel, resolving then dropping the
/// routing guard (never holding `conns` while locking the channel).
async fn resolve_channel(
    state: &ServerState,
    name: &ChannelName,
) -> Option<Arc<Mutex<ChannelState>>> {
    let channels = state.channels.read().await;
    channels.get(name).cloned()
}

/// Create (or fetch, re-verifying under the write lock) a channel.
/// Safe double-checked locking: a racing Attach that saw a reaped Arc is
/// redirected to the canonical instance instead of overwriting it.
async fn get_or_create_channel(
    state: &SharedState,
    name: &ChannelName,
) -> Arc<Mutex<ChannelState>> {
    {
        let channels = state.channels.read().await;
        if let Some(existing) = channels.get(name) {
            let arc = existing.clone();
            drop(channels);
            let is_reaped = arc.lock().await.is_reaped;
            if !is_reaped {
                return arc;
            }
        }
    }

    let mut channels = state.channels.write().await;
    // Re-check under the write lock: a racing thread may have inserted a
    // non-reaped channel between our read and write acquisition. A reaped
    // (or absent) entry falls through and is replaced with a fresh channel.
    if let Some(existing) = channels.get(name) {
        let arc = existing.clone();
        let is_reaped = arc.lock().await.is_reaped;
        if !is_reaped {
            return arc;
        }
    }
    let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>(INPUT_CHANNEL_CAPACITY);
    let notify = Arc::new(Notify::new());
    let created_seq = state.next_channel_seq.fetch_add(1, Ordering::Relaxed);
    let channel = Arc::new(Mutex::new(ChannelState::new(
        Vec::new(),
        input_tx,
        notify,
        created_seq,
    )));
    let ch = Arc::clone(&channel);
    tokio::spawn(async move {
        let mut input_rx = input_rx;
        while let Some(data) = input_rx.recv().await {
            let writer = {
                let guard = ch.lock().await;
                guard.session.as_ref().map(|s| s.pty.writer_handle())
            };
            if let Some(writer) = writer {
                let _ = tokio::task::spawn_blocking(move || writer.write_bytes(&data)).await;
            }
        }
        // Note: the input task runs for the daemon lifetime; the channel's
        // `input_rx` is only dropped when the channel's senders all vanish.
    });

    // Output polling task: drains PTY output and broadcasts to subscribers;
    // on session exit finalizes subscribers, clears the session, and reaps
    // the channel if no clients remain.
    {
        let st = Arc::clone(state);
        let ch = Arc::clone(&channel);
        let notify = {
            let locked = ch.lock().await;
            locked.notify.clone()
        };
        let name_for_task = name.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = notify.notified() => {}
                    _ = tokio::time::sleep(SESSION_EXIT_POLL_INTERVAL) => {}
                }
                let mut guard = ch.lock().await;
                if guard.is_reaped {
                    break;
                }
                if guard.subscribers.is_empty() {
                    if let Some(session) = guard.session.as_mut() {
                        session.sync_screen();
                        if session.check_exited() {
                            tracing::info!(channel = %name_for_task, "Session exited");
                            guard.session = None;
                            guard.kill_pending = false;
                        }
                    }
                } else {
                    let (raw, exited, code) = {
                        let Some(session) = guard.session.as_mut() else {
                            // No live session: finalize any lingering subscribers.
                            for sub in &guard.subscribers {
                                sub.respond.respond(Vec::new(), true);
                            }
                            guard.subscribers.clear();
                            guard.notify.notify_one();
                            continue;
                        };
                        let raw = session.read_output();
                        let exited = session.check_exited();
                        let code = session.exit_code;
                        (raw, exited, code)
                    };
                    if !raw.is_empty() {
                        for sub in &guard.subscribers {
                            sub.respond.respond(raw.clone(), false);
                        }
                    }
                    if exited {
                        tracing::info!(channel = %name_for_task, "Session exited with code {:?}", code);
                        for sub in &guard.subscribers {
                            sub.respond.respond(Vec::new(), true);
                        }
                        guard.subscribers.clear();
                        guard.session = None;
                        guard.kill_pending = false;
                        guard.notify.notify_one();
                    }
                }
                let should_reap = guard.session.is_none() && guard.clients.is_empty();
                drop(guard);

                if should_reap {
                    // GC: drop the channel guard before requesting `channels.write`
                    // (strict ordering, no AB-BA), then re-verify under the write lock.
                    let mut channels = st.channels.write().await;
                    if let Some(arc) = channels.get(&name_for_task) {
                        let mut locked = arc.lock().await;
                        if locked.session.is_none() && locked.clients.is_empty() {
                            locked.is_reaped = true;
                            drop(locked);
                            channels.remove(&name_for_task);
                            tracing::info!(channel = %name_for_task, "Reaped idle channel");
                        }
                    }
                }
                // Note: the daemon deliberately persists until an explicit
                // `ShutdownGateway` / `term-session stop`. Sessions survive
                // client disconnects; idle channels are reaped above but the
                // gateway process itself is never torn down implicitly.
            }
        });
    }

    channels.insert(name.clone(), Arc::clone(&channel));
    channel
}

/// Remove a connection from the routing table and prune it from its bound
/// channel's client/subscriber maps (authoritative teardown on disconnect).
async fn evict_conn(state: &ServerState, conn_id: usize) {
    let channel = {
        let mut conns = state.conns.write().await;
        let entry = conns.remove(&conn_id);
        entry.and_then(|e| match e.state {
            ConnState::Attached(name) => Some(name),
            ConnState::Unattached => None,
        })
    };
    let Some(channel) = channel else {
        return;
    };
    let Some(ch) = resolve_channel(state, &channel).await else {
        return;
    };
    let mut guard = ch.lock().await;
    guard.clients.remove(&conn_id);
    guard.subscribers.retain(|s| s.conn_id != conn_id);
    guard.recalculate_pty_size();
    // Broadcast the session's actual (client-driven) geometry to remaining
    // clients. If no session exists there is nothing to broadcast.
    let session_size = guard.session.as_ref().map(|s| (s.cols, s.rows));
    let targets: Vec<ClientEntry> = guard.clients.values().cloned().collect();
    drop(guard);
    let Some((ncols, nrows)) = session_size else {
        return;
    };
    // Best-effort geometry broadcast; the channel may be reaped concurrently,
    // in which case clients already see end-of-stream.
    if let Some(ch) = resolve_channel(state, &channel).await {
        let guard = ch.lock().await;
        guard.notify_clients(&targets, ncols, nrows);
    }
}

/// Spawn a detached escalation task for a kill-requested session, returning
/// its `JoinHandle` so the caller (e.g. shutdown teardown) can await it.
///
/// Policy owned by the daemon supervisor (never the pty engine): after an
/// async `SIGKILL_GRACE` sleep, re-check the session state; if it already
/// exited during the grace window (reaped by the output-polling task), abort
/// instantly — no blind `SIGKILL` to a possibly-recycled pgid. Only escalate
/// if the session is still alive and the kill is still pending.
async fn spawn_kill_escalation(
    state: &SharedState,
    name: &ChannelName,
) -> tokio::task::JoinHandle<()> {
    let state = Arc::clone(state);
    let name = name.clone();
    tokio::spawn(async move {
        tokio::time::sleep(SIGKILL_GRACE).await;
        let Some(ch) = resolve_channel(&state, &name).await else {
            return;
        };
        let mut guard = ch.lock().await;
        if !guard.kill_pending {
            return;
        }
        // Abort if the session exited during the grace (or is already gone).
        let alive = guard
            .session
            .as_ref()
            .is_some_and(|s| !s.exited && s.pty.reader_is_alive());
        guard.kill_pending = false;
        if !alive {
            return;
        }
        if let Some(session) = guard.session.as_mut() {
            #[cfg(unix)]
            let _ = session.pty.signal_process_group(SIGKILL);
            #[cfg(not(unix))]
            let _ = session.pty.kill_child();
        }
    })
}

/// Run the gateway daemon. Hosts every channel in one process; returns after
/// a `ShutdownGateway` (or transport error).
pub async fn run_gateway(
    gateway: ChannelName,
) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
    let socket_name = gateway.to_string();
    let state: SharedState = Arc::new(ServerState {
        conns: RwLock::new(HashMap::new()),
        channels: RwLock::new(HashMap::new()),
        is_shutting_down: AtomicBool::new(false),
        next_channel_seq: AtomicU64::new(0),
    });

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let server = RpcIpcServer::new(Some(event_tx));
    let endpoint = server.endpoint();

    // ── Attach ────────────────────────────────────────────────────────
    let st = Arc::clone(&state);
    endpoint
        .register_prebuffered(Attach::METHOD_ID, move |payload, ctx| {
            let state = Arc::clone(&st);
            async move {
                if state.is_shutting_down.load(Ordering::SeqCst) {
                    return Err(rpc_err(RPC_ERROR_SHUTTING_DOWN));
                }
                let (channel_str, hostname, pid, user, version, ssh_ip) =
                    Attach::decode_request(&payload)?;
                let name = ChannelName::parse(&channel_str).map_err(|e| rpc_err(&e))?;
                let _channel = get_or_create_channel(&state, &name).await;
                let conn_id = ctx.conn_id;
                let mut conns = state.conns.write().await;
                // The `ClientConnected` event is processed by a separate async
                // loop, so it may not have inserted the entry yet when this
                // handler runs. Ensure the entry exists before binding.
                let entry = conns.entry(conn_id).or_insert_with(|| ConnEntry {
                    handle: RpcIpcConnectionContextHandle(ctx.clone()),
                    state: ConnState::Unattached,
                    hostname: String::new(),
                    connected_at_unix: now_unix(),
                    pid: 0,
                    user: String::new(),
                    version: String::new(),
                    ssh_ip: None,
                });
                entry.state = ConnState::Attached(name);
                entry.hostname = hostname;
                entry.connected_at_unix = now_unix();
                entry.pid = pid;
                entry.user = user;
                entry.version = version;
                entry.ssh_ip = ssh_ip;
                Attach::encode_response(conn_id).map_err(boxed_io)
            }
        })
        .await
        .map_err(|e| format!("register Attach: {e:?}"))?;

    // ── Spawn ────────────────────────────────────────────────────────
    let st = Arc::clone(&state);
    endpoint
        .register_prebuffered(Spawn::METHOD_ID, move |payload, ctx| {
            let state = Arc::clone(&st);
            async move {
                if state.is_shutting_down.load(Ordering::SeqCst) {
                    return Err(rpc_err(RPC_ERROR_SHUTTING_DOWN));
                }
                let (cmd, cols, rows) = Spawn::decode_request(&payload)?;
                let channel = bound_channel(state.as_ref(), ctx.conn_id).await;
                let Some(channel) = channel else {
                    return Err(rpc_err(RPC_ERROR_UNATTACHED));
                };
                let ch = get_or_create_channel(&state, &channel).await;
                // Fetch the connection's caller handle, hostname, and connect
                // time before locking the channel (never hold two locks).
                let conn_meta = {
                    let conns = state.conns.read().await;
                    conns.get(&ctx.conn_id).map(|c| {
                        (
                            c.handle.clone(),
                            c.hostname.clone(),
                            c.connected_at_unix,
                            c.pid,
                            c.user.clone(),
                            c.version.clone(),
                            c.ssh_ip.clone(),
                        )
                    })
                };
                let mut guard = ch.lock().await;
                let entry = guard
                    .clients
                    .entry(ctx.conn_id)
                    .or_insert_with(|| ClientEntry {
                        caller: conn_meta.as_ref().map(|(h, _, _, _, _, _, _)| h.clone()),
                        hostname: conn_meta
                            .as_ref()
                            .map(|(_, n, _, _, _, _, _)| n.clone())
                            .unwrap_or_default(),
                        connected_at_unix: conn_meta.as_ref().map(|(_, _, t, _, _, _, _)| *t).unwrap_or(0),
                        pid: conn_meta.as_ref().map(|(_, _, _, p, _, _, _)| *p).unwrap_or(0),
                        user: conn_meta
                            .as_ref()
                            .map(|(_, _, _, _, u, _, _)| u.clone())
                            .unwrap_or_default(),
                        version: conn_meta
                            .as_ref()
                            .map(|(_, _, _, _, _, v, _)| v.clone())
                            .unwrap_or_default(),
                        ssh_ip: conn_meta.as_ref().and_then(|(_, _, _, _, _, _, s)| s.clone()),
                        cols,
                        rows,
                    });
                entry.cols = cols;
                entry.rows = rows;

                // If a session already exists and hasn't exited, reuse it.
                if guard.session.as_ref().is_some_and(|s| !s.exited) {
                    guard.recalculate_pty_size();
                    let session = guard.session.as_ref().unwrap();
                    let (ncols, nrows) = (session.cols, session.rows);
                    let targets: Vec<ClientEntry> = guard.clients.values().cloned().collect();
                    let id = session.id;
                    let cols = session.cols;
                    let rows = session.rows;
                    drop(guard);
                    if let Some(ch) = resolve_channel(state.as_ref(), &channel).await {
                        let g = ch.lock().await;
                        g.notify_clients(&targets, ncols, nrows);
                    }
                    return Spawn::encode_response((id, cols, rows)).map_err(boxed_io);
                }

                // Respawn: new non-empty cmd overwrites the stored template;
                // empty/None falls back to the existing template.
                let effective_cmd = if let Some(c) = cmd
                    && !c.is_empty()
                {
                    guard.cmd = c.clone();
                    Some(c)
                } else if !guard.cmd.is_empty() {
                    Some(guard.cmd.clone())
                } else {
                    None
                };
                let id = SESSION_ID;
                let session = Session::spawn(id, effective_cmd, cols, rows, Some(&channel))?;
                guard.set_session(session);
                guard.recalculate_pty_size();
                let targets: Vec<ClientEntry> = guard.clients.values().cloned().collect();
                let session = guard.session.as_ref().unwrap();
                let (sid, scol, srow) = (session.id, session.cols, session.rows);
                let (ncols, nrows) = (scol, srow);
                drop(guard);
                if let Some(ch) = resolve_channel(state.as_ref(), &channel).await {
                    let g = ch.lock().await;
                    g.notify_clients(&targets, ncols, nrows);
                }
                Spawn::encode_response((sid, scol, srow)).map_err(boxed_io)
            }
        })
        .await
        .map_err(|e| format!("register Spawn: {e:?}"))?;

    // ── ResizePty ────────────────────────────────────────────────────
    let st = Arc::clone(&state);
    endpoint
        .register_prebuffered(ResizePty::METHOD_ID, move |payload, ctx| {
            let state = Arc::clone(&st);
            async move {
                if state.is_shutting_down.load(Ordering::SeqCst) {
                    return Err(rpc_err(RPC_ERROR_SHUTTING_DOWN));
                }
                let (_id, cols, rows) = ResizePty::decode_request(&payload)?;
                let channel = bound_channel(state.as_ref(), ctx.conn_id).await;
                let Some(channel) = channel else {
                    return Err(rpc_err(RPC_ERROR_UNATTACHED));
                };
                let ch = resolve_channel(state.as_ref(), &channel)
                    .await
                    .ok_or_else(|| rpc_err("channel not found"))?;
                let mut guard = ch.lock().await;
                if let Some(client) = guard.clients.get_mut(&ctx.conn_id) {
                    client.cols = cols;
                    client.rows = rows;
                }
                guard.recalculate_pty_size();
                let (ncols, nrows) = guard
                    .session
                    .as_ref()
                    .map(|s| (s.cols, s.rows))
                    .unwrap_or((cols, rows));
                let targets: Vec<ClientEntry> = guard.clients.values().cloned().collect();
                drop(guard);
                if let Some(ch) = resolve_channel(state.as_ref(), &channel).await {
                    let g = ch.lock().await;
                    g.notify_clients(&targets, ncols, nrows);
                }
                ResizePty::encode_response((ncols, nrows)).map_err(boxed_io)
            }
        })
        .await
        .map_err(|e| format!("register ResizePty: {e:?}"))?;

    // ── CloseSession ─────────────────────────────────────────────────
    let st = Arc::clone(&state);
    endpoint
        .register_prebuffered(CloseSession::METHOD_ID, move |payload, ctx| {
            let state = Arc::clone(&st);
            async move {
                if state.is_shutting_down.load(Ordering::SeqCst) {
                    return Err(rpc_err(RPC_ERROR_SHUTTING_DOWN));
                }
                let _id = CloseSession::decode_request(&payload)?;
                let channel = bound_channel(state.as_ref(), ctx.conn_id).await;
                let Some(channel) = channel else {
                    return Err(rpc_err(RPC_ERROR_UNATTACHED));
                };
                let ch = resolve_channel(state.as_ref(), &channel)
                    .await
                    .ok_or_else(|| rpc_err("channel not found"))?;
                let mut guard = ch.lock().await;
                guard.request_session_kill(SIGTERM);
                guard.finalize_subscribers();
                drop(guard);
                // Spawn the exited-checked SIGKILL escalation for stragglers.
                spawn_kill_escalation(&state, &channel).await;
                CloseSession::encode_response(()).map_err(boxed_io)
            }
        })
        .await
        .map_err(|e| format!("register CloseSession: {e:?}"))?;

    // ── WriteInput ───────────────────────────────────────────────────
    let st = Arc::clone(&state);
    endpoint
        .register_prebuffered(WriteInput::METHOD_ID, move |payload, ctx| {
            let state = Arc::clone(&st);
            async move {
                if state.is_shutting_down.load(Ordering::SeqCst) {
                    return Err(rpc_err(RPC_ERROR_SHUTTING_DOWN));
                }
                let (id, data) = WriteInput::decode_request(&payload)?;
                let channel = bound_channel(state.as_ref(), ctx.conn_id).await;
                let Some(channel) = channel else {
                    return Err(rpc_err(RPC_ERROR_UNATTACHED));
                };
                let ch = resolve_channel(state.as_ref(), &channel)
                    .await
                    .ok_or_else(|| rpc_err("channel not found"))?;
                let writer = {
                    let guard = ch.lock().await;
                    guard
                        .session
                        .as_ref()
                        .filter(|s| s.id == id)
                        .map(|s| s.pty.writer_handle())
                };
                // PTY writes are blocking I/O (kernel input buffer); offload
                // to the blocking pool so a full buffer never stalls an async
                // worker or holds the state lock.
                if let Some(writer) = writer {
                    let _ = tokio::task::spawn_blocking(move || writer.write_bytes(&data)).await;
                }
                WriteInput::encode_response(()).map_err(boxed_io)
            }
        })
        .await
        .map_err(|e| format!("register WriteInput: {e:?}"))?;

    // ── StreamInput ──────────────────────────────────────────────────
    let st = Arc::clone(&state);
    endpoint
        .register_stream_handler(STREAM_INPUT_METHOD_ID, move |event, _responder, ctx| {
            if let RpcStreamEvent::PayloadChunk { bytes, .. } = event {
                let state = Arc::clone(&st);
                let conn_id = ctx.conn_id;
                tokio::spawn(async move {
                    let channel = bound_channel(state.as_ref(), conn_id).await;
                    let Some(channel) = channel else {
                        return;
                    };
                    let ch = resolve_channel(state.as_ref(), &channel).await;
                    let Some(ch) = ch else {
                        return;
                    };
                    let tx = {
                        let guard = ch.lock().await;
                        guard.input_tx.clone()
                    };
                    if let Err(e) = tx.try_send(bytes) {
                        tracing::warn!(error = %e, "gateway input buffer full; dropping input chunk");
                    }
                });
            }
            // Intentionally ignore End/Error — the channel stays alive.
        })
        .await
        .map_err(|e| format!("register stream handler STREAM_INPUT: {e:?}"))?;

    // ── SubscribeOutput ──────────────────────────────────────────────
    let st = Arc::clone(&state);
    endpoint
        .register_stream_handler(SUBSCRIBE_OUTPUT_METHOD_ID, move |event, respond, ctx| {
            let is_new = matches!(&event, RpcStreamEvent::Header { .. });
            if is_new {
                let st = Arc::clone(&st);
                let conn_id = ctx.conn_id;
                tokio::spawn(async move {
                    let channel = bound_channel(&st, conn_id).await;
                    let Some(channel) = channel else {
                        return;
                    };
                    let ch = resolve_channel(&st, &channel).await;
                    let Some(ch) = ch else {
                        return;
                    };
                    let mut guard = ch.lock().await;
                    // Drain accumulated PTY output and capture the raw bytes
                    // so they can be sent to the new subscriber.
                    let early = guard.session.as_mut().and_then(|s| {
                        let data = s.read_output();
                        if data.is_empty() { None } else { Some(data) }
                    });
                    let snapshot = guard.session.as_mut().map(|s| s.generate_snapshot());
                    guard.subscribers.push(SubscriberEntry {
                        conn_id,
                        respond: respond.clone(),
                    });
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

    // ── ListChannels ─────────────────────────────────────────────────
    let st = Arc::clone(&state);
    let list_socket = socket_name.clone();
    endpoint
        .register_prebuffered(ListChannels::METHOD_ID, move |_payload, _ctx| {
            let state = Arc::clone(&st);
            let socket = list_socket.clone();
            async move {
                let channels = {
                    let chans = state.channels.read().await;
                    chans.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>()
                };
                // Creation order, newest last (monotonic `created_seq`, not
                // second-resolution wall clock). The seq is read under the same
                // per-channel lock used to build `ChannelInfo` below.
                let mut out: Vec<(u64, ChannelInfo)> = Vec::with_capacity(channels.len());
                for (name, ch) in channels {
                    let guard = ch.lock().await;
                    out.push((guard.created_seq, guard.to_info(&name)));
                }
                out.sort_by_key(|(seq, _)| *seq);
                let out: Vec<ChannelInfo> = out.into_iter().map(|(_, info)| info).collect();
                ListChannels::encode_response(ListChannelsResponse {
                    gateway_pid: std::process::id() as u64,
                    socket,
                    channels: out,
                })
                .map_err(boxed_io)
            }
        })
        .await
        .map_err(|e| format!("register ListChannels: {e:?}"))?;

    // ── KillChannel ──────────────────────────────────────────────────
    let st = Arc::clone(&state);
    endpoint
        .register_prebuffered(KillChannel::METHOD_ID, move |payload, _ctx| {
            let state = Arc::clone(&st);
            async move {
                if state.is_shutting_down.load(Ordering::SeqCst) {
                    return Err(rpc_err(RPC_ERROR_SHUTTING_DOWN));
                }
                let channel_str = KillChannel::decode_request(&payload)?;
                let name = ChannelName::parse(&channel_str).map_err(|e| rpc_err(&e))?;
                // 1) Snapshot the target connections under `conns.write`, then release.
                let target_conns: Vec<usize> = {
                    let conns = state.conns.read().await;
                    conns
                        .iter()
                        .filter(|(_, entry)| matches!(entry.state, ConnState::Attached(ref n) if n == &name))
                        .map(|(conn_id, _)| *conn_id)
                        .collect()
                };
                // 2) Lock the channel: signal the session tree and evict every socket.
                if let Some(ch) = resolve_channel(state.as_ref(), &name).await {
                    let mut guard = ch.lock().await;
                    guard.request_session_kill(SIGTERM);
                    guard.finalize_subscribers();
                    for conn_id in &target_conns {
                        guard.clients.remove(conn_id);
                        guard.subscribers.retain(|s| s.conn_id != *conn_id);
                    }
                    drop(guard);
                    spawn_kill_escalation(&state, &name).await;
                }
                // 3) Evict the ConnEntry records from the routing table.
                let mut conns = state.conns.write().await;
                for conn_id in &target_conns {
                    conns.remove(conn_id);
                }
                KillChannel::encode_response(()).map_err(boxed_io)
            }
        })
        .await
        .map_err(|e| format!("register KillChannel: {e:?}"))?;

    // ── KillClient ───────────────────────────────────────────────────
    let st = Arc::clone(&state);
    endpoint
        .register_prebuffered(KillClient::METHOD_ID, move |payload, _ctx| {
            let state = Arc::clone(&st);
            async move {
                if state.is_shutting_down.load(Ordering::SeqCst) {
                    return Err(rpc_err(RPC_ERROR_SHUTTING_DOWN));
                }
                let (channel_str, conn_id) = KillClient::decode_request(&payload)?;
                let name = ChannelName::parse(&channel_str).map_err(|e| rpc_err(&e))?;
                // Reject if the conn does not exist or is not attached to the
                // named channel — kill-client must target a real client.
                let bound_channel = {
                    let conns = state.conns.read().await;
                    conns.get(&conn_id).and_then(|c| match &c.state {
                        ConnState::Attached(n) if n == &name => Some(n.clone()),
                        _ => None,
                    })
                };
                let Some(bound) = bound_channel else {
                    return Err(rpc_err(&format!(
                        "client {conn_id} is not attached to channel '{name}'"
                    )));
                };
                // Evict the conn first (conns → channel ordering).
                {
                    let mut conns = state.conns.write().await;
                    conns.remove(&conn_id);
                }
                if let Some(ch) = resolve_channel(state.as_ref(), &bound).await {
                    let mut guard = ch.lock().await;
                    // End the evicted subscriber's stream before dropping it.
                    let mut evicted: Vec<StreamResponder> = Vec::new();
                    let mut keep = Vec::with_capacity(guard.subscribers.len());
                    for sub in guard.subscribers.drain(..) {
                        if sub.conn_id == conn_id {
                            evicted.push(sub.respond);
                        } else {
                            keep.push(sub);
                        }
                    }
                    guard.subscribers = keep;
                    for respond in evicted {
                        respond.respond(Vec::new(), true);
                    }
                    guard.clients.remove(&conn_id);
                    guard.recalculate_pty_size();
                    let session_size = guard.session.as_ref().map(|s| (s.cols, s.rows));
                    let targets: Vec<ClientEntry> = guard.clients.values().cloned().collect();
                    drop(guard);
                    if let (Some((ncols, nrows)), Some(ch)) =
                        (session_size, resolve_channel(state.as_ref(), &bound).await)
                    {
                        let g = ch.lock().await;
                        g.notify_clients(&targets, ncols, nrows);
                    }
                }
                KillClient::encode_response(()).map_err(boxed_io)
            }
        })
        .await
        .map_err(|e| format!("register KillClient: {e:?}"))?;

    // ── ShutdownGateway ──────────────────────────────────────────────
    let st = Arc::clone(&state);
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));
    endpoint
        .register_prebuffered(ShutdownGateway::METHOD_ID, move |payload, _ctx| {
            let state = Arc::clone(&st);
            let shutdown_tx = Arc::clone(&shutdown_tx);
            async move {
                // Refuse an accidental shutdown while live sessions are running
                // unless the caller explicitly forced it. Checked BEFORE the
                // `is_shutting_down` seal so a refused stop leaves the gateway
                // fully operational (no half-sealed state, no orphaned teardown).
                let force = ShutdownGateway::decode_request(&payload).map_err(boxed_io)?;
                if !force {
                    let live = {
                        let chans = state.channels.read().await;
                        let mut n = 0usize;
                        for ch in chans.values() {
                            let guard = ch.lock().await;
                            if guard.session.as_ref().is_some_and(|s| !s.exited) {
                                n += 1;
                            }
                        }
                        n
                    };
                    if live > 0 {
                        return Err(rpc_err(&format!(
                            "{RPC_ERROR_LIVE_SESSIONS} ({live} live session(s))"
                        )));
                    }
                }
                // Atomic seal: reject all further RPCs before teardown starts.
                state.is_shutting_down.store(true, Ordering::SeqCst);
                // Snapshot the channels, then release the map lock.
                let channels: Vec<(ChannelName, Arc<Mutex<ChannelState>>)> = {
                    let chans = state.channels.read().await;
                    chans.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                };
                // Off-lock: signal each session's process group (non-blocking),
                // finalize subscribers, and collect the escalation handles so
                // the exit signal fires only after every child tree is reaped.
                let mut escalations = Vec::new();
                for (name, ch) in channels {
                    let mut guard = ch.lock().await;
                    tracing::info!(channel = %name, "Shutdown: signaling session tree");
                    guard.request_session_kill(SIGTERM);
                    guard.finalize_subscribers();
                    drop(guard);
                    escalations.push(spawn_kill_escalation(&state, &name).await);
                }
                // Deferred exit signal: await the SIGKILL escalation tasks so
                // all child process trees are definitively terminated, then
                // sleep a grace so the transport flushes the `()` response
                // frame, then fire the oneshot that ends run_gateway. Never
                // fire it synchronously from the handler.
                tokio::spawn(async move {
                    for handle in escalations {
                        let _ = handle.await;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(SHUTDOWN_FLUSH_GRACE_MS))
                        .await;
                    let mut tx_guard = shutdown_tx.lock().await;
                    if let Some(tx) = tx_guard.take() {
                        let _ = tx.send(());
                    }
                });
                ShutdownGateway::encode_response(()).map_err(boxed_io)
            }
        })
        .await
        .map_err(|e| format!("register ShutdownGateway: {e:?}"))?;

    // ── Connection event loop ────────────────────────────────────────
    let st = Arc::clone(&state);
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                RpcIpcServerEvent::ClientConnected(handle) => {
                    tracing::info!("Client {} connected", handle.0.conn_id);
                    let mut conns = st.conns.write().await;
                    // A fast client may send Attach before this event is
                    // processed; the Attach handler already inserted an
                    // `Attached` entry. `insert` would clobber that binding and
                    // a subsequent Spawn would see the state reset to
                    // Unattached. Only create the entry if it is absent.
                    conns.entry(handle.0.conn_id).or_insert_with(|| ConnEntry {
                        handle: handle.clone(),
                        state: ConnState::Unattached,
                        hostname: String::new(),
                        connected_at_unix: now_unix(),
                        pid: 0,
                        user: String::new(),
                        version: String::new(),
                        ssh_ip: None,
                    });
                }
                RpcIpcServerEvent::ClientDisconnected(conn_id) => {
                    tracing::info!("Client {conn_id} disconnected");
                    evict_conn(st.as_ref(), conn_id).await;
                }
            }
        }
    });

    tracing::info!("Gateway listening on channel {gateway}");

    // Wait for either the server to finish or a shutdown signal.
    let exit_code = tokio::select! {
        result = async {
            server.serve(&socket_name).await.map_err(|e| format!("serve: {e:?}"))
        } => {
            result?;
            0
        }
        _ = &mut shutdown_rx => {
            // Give the transport time to flush final subscriber frames.
            tokio::time::sleep(SESSION_EXIT_FLUSH_GRACE).await;
            0
        }
    };

    Ok(exit_code)
}
