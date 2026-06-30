use std::io::{self};
use std::net::TcpStream;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender, TryRecvError},
};
use std::thread;
use std::time::Duration;

use term_session_server::protocol::{
    self, SessionServerPush, SessionServerRequest, SessionServerResponse,
};

enum NetCmd {
    Write {
        id: u64,
        data: Vec<u8>,
    },
    Request {
        req: SessionServerRequest,
        resp: Sender<io::Result<SessionServerResponse>>,
    },
}

/// Sender half — all TCP I/O happens in a background thread so the
/// event loop can never block on a network write.
/// Cloning gives another independent sender; the net thread lives
/// until all senders and the [`SessionServerReceiver`] are dropped.
#[derive(Clone)]
pub struct SessionServerConnection {
    cmd_tx: Sender<NetCmd>,
    is_alive: Arc<AtomicBool>,
}

impl SessionServerConnection {
    /// Connect to a term-session-server.
    ///
    /// Returns a connected sender + a receiver for inbound pushes.
    /// The background net thread handles all TCP reads and writes.
    pub fn connect(addr: &str) -> io::Result<(Self, SessionServerReceiver)> {
        let stream = TcpStream::connect(addr)?;

        let (cmd_tx, cmd_rx) = mpsc::channel::<NetCmd>();
        let (push_tx, push_rx) = mpsc::channel::<SessionServerPush>();
        let is_alive = Arc::new(AtomicBool::new(true));
        let alive = Arc::clone(&is_alive);

        thread::Builder::new()
            .name("session-server-net".into())
            .spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                let mut stream = stream;
                let mut pending: Option<Sender<io::Result<SessionServerResponse>>> = None;

                let respond = |tx: &Sender<_>, result| {
                    let _ = tx.send(result);
                };

                loop {
                    // Drain all pending commands (non-blocking).
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        match cmd {
                            NetCmd::Write { id, data } => {
                                let req = SessionServerRequest::Write { id, data };
                                let payload = bitcode::encode(&req);
                                let _ = protocol::send_msg(
                                    &mut stream,
                                    protocol::MSG_REQUEST,
                                    &payload,
                                );
                            }
                            NetCmd::Request { req, resp } => {
                                pending = None;

                                let payload = bitcode::encode(&req);
                                match protocol::send_msg(
                                    &mut stream,
                                    protocol::MSG_REQUEST,
                                    &payload,
                                ) {
                                    Ok(()) => pending = Some(resp),
                                    Err(e) => {
                                        respond(&resp, Err(e));
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    // Check whether the cmd channel was disconnected
                    // (all SessionServerConnection clones dropped).  If so, exit.
                    if matches!(cmd_rx.try_recv(), Err(TryRecvError::Disconnected)) {
                        break;
                    }

                    // Read one message from TCP.
                    match protocol::recv_msg(&mut stream) {
                        Ok((protocol::MSG_PUSH, payload)) => {
                            if let Ok(push) = bitcode::decode::<SessionServerPush>(&payload) {
                                let _ = push_tx.send(push);
                            }
                        }
                        Ok((protocol::MSG_RESPONSE, payload)) => {
                            let response =
                                bitcode::decode(&payload).unwrap_or(SessionServerResponse::Error {
                                    msg: "decode failed".into(),
                                });
                            if let Some(tx) = pending.take() {
                                respond(&tx, Ok(response));
                            }
                        }
                        Ok(_) => {}
                        Err(ref e)
                            if e.kind() == io::ErrorKind::TimedOut
                                || e.kind() == io::ErrorKind::WouldBlock => {}
                        Err(e) => {
                            if let Some(tx) = pending.take() {
                                respond(&tx, Err(io::Error::other(e.to_string())));
                            }
                            break;
                        }
                    }
                }

                alive.store(false, Ordering::Relaxed);
            })
            .map_err(io::Error::other)?;

        Ok((
            SessionServerConnection {
                cmd_tx,
                is_alive: is_alive.clone(),
            },
            SessionServerReceiver { push_rx, is_alive },
        ))
    }

    /// Fire-and-forget write — returns immediately.  The write is queued
    /// in the background net thread's channel and sent to the server as
    /// soon as possible.
    pub fn send_write(&self, id: u64, data: &[u8]) -> io::Result<()> {
        if !self.is_alive() {
            return Err(io::Error::other("session server connection lost"));
        }
        self.cmd_tx
            .send(NetCmd::Write {
                id,
                data: data.to_vec(),
            })
            .map_err(|_| io::Error::other("daemon net thread died"))
    }

    /// Send a request and wait for the server's response (up to 5 s).
    pub fn send_request(&self, req: &SessionServerRequest) -> io::Result<SessionServerResponse> {
        if !self.is_alive() {
            return Err(io::Error::other("session server connection lost"));
        }
        let (tx, rx) = mpsc::channel();
        self.cmd_tx
            .send(NetCmd::Request {
                req: req.clone(),
                resp: tx,
            })
            .map_err(|_| io::Error::other("daemon net thread died"))?;
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| io::Error::other("send_request timeout"))?
    }

    pub fn is_alive(&self) -> bool {
        self.is_alive.load(Ordering::Relaxed)
    }
}

/// Receiver half — drains inbound pushes published by the background
/// net thread.  Not cloneable (the push channel is single-consumer).
pub struct SessionServerReceiver {
    push_rx: Receiver<SessionServerPush>,
    is_alive: Arc<AtomicBool>,
}

impl SessionServerReceiver {
    /// Drain all buffered pushes without blocking.
    pub fn drain_pushes(&mut self) -> Vec<SessionServerPush> {
        let mut pushes = Vec::new();
        while let Ok(push) = self.push_rx.try_recv() {
            pushes.push(push);
        }
        pushes
    }

    pub fn is_alive(&self) -> bool {
        self.is_alive.load(Ordering::Relaxed)
    }
}
