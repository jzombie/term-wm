use std::io::{self};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use crate::protocol::{self, SessionServerPush, SessionServerRequest, SessionServerResponse};
use crate::session::Session;

pub struct SessionServerConfig {
    pub bind_addr: String,
    pub cmd: Vec<String>,
    pub cols: u16,
    pub rows: u16,
}

pub struct SessionServer {
    config: SessionServerConfig,
    session: Option<Session>,
    exit_code: i32,
    client_connected: bool,
}

impl SessionServer {
    pub fn new(config: SessionServerConfig) -> Self {
        Self {
            config,
            session: None,
            exit_code: 0,
            client_connected: false,
        }
    }

    /// The exit code from the child process that exited, or 0.
    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    /// Poll the single session for child-exit, capturing the exit code.
    /// Returns true once the session has exited.
    fn poll_exits(&mut self) -> bool {
        let Some(session) = self.session.as_mut() else {
            return true;
        };
        if session.check_exited() {
            let status = session
                .pty
                .exit_status()
                .map_or(-1, |s| s.exit_code() as i32);
            if self.exit_code == 0 {
                self.exit_code = status;
            }
        }
        session.exited
    }

    pub fn run(&mut self) -> io::Result<()> {
        let cmd = if self.config.cmd.is_empty() {
            None
        } else {
            Some(self.config.cmd.clone())
        };
        let session = Session::spawn(1, cmd, self.config.cols, self.config.rows)
            .map_err(|e| io::Error::other(e.to_string()))?;
        self.session = Some(session);

        // Brief retry loop: let quick commands finish before we block on accept().
        for _ in 0..10 {
            if self.poll_exits() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let listener = TcpListener::bind(&self.config.bind_addr)?;
        listener.set_nonblocking(true)?;
        tracing::info!("Server listening on {}", self.config.bind_addr);

        loop {
            if self.poll_exits() {
                return Ok(());
            }

            let mut client = match listener.accept() {
                Ok((stream, addr)) => {
                    // IMPORTANT: This *must* be set to false. When it is set to true
                    // the latency starts stacking up nearly immediately on macOS making
                    // it impossible to type.
                    //
                    // The accepted stream may inherit non-blocking mode from
                    // the listener on some platforms (macOS/Darwin), which
                    // would break protocol reads.  Restore blocking mode.
                    stream.set_nonblocking(false)?;

                    tracing::info!("Client connected: {addr}");
                    stream.set_read_timeout(Some(Duration::from_millis(16)))?;
                    stream.set_write_timeout(Some(Duration::from_millis(100)))?;
                    stream
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Err(e) => {
                    tracing::error!("accept failed: {e}");
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
            };

            // If a client is already connected, refuse any new connection.
            if self.client_connected {
                tracing::info!("Refusing connection — already have a client");
                continue;
            }

            self.client_connected = true;
            self.send_welcome(&mut client)?;
            self.send_snapshot(&mut client)?;

            'connected: loop {
                // Interleave: process at most 10 commands, then check PTY output.
                for _ in 0..10 {
                    match self.process_client(&mut client) {
                        Ok(true) => {}
                        Ok(false) => break,
                        Err(e) => {
                            tracing::info!("Client disconnected: {e}");
                            break 'connected;
                        }
                    }
                }

                let had_output = match self.push_session_updates(&mut client) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::info!("Client disconnected: {e}");
                        break 'connected;
                    }
                };

                if self.poll_exits() {
                    break 'connected;
                }

                if had_output {
                    continue;
                }
                std::thread::sleep(Duration::from_millis(4));
            }

            self.client_connected = false;
        }
    }

    /// Returns Ok(true) if a command was processed, Ok(false) if no command
    /// was available (timed out / would block), or Err on disconnect.
    fn process_client(&mut self, client: &mut TcpStream) -> io::Result<bool> {
        let (msg_type, payload) = match protocol::recv_msg(client) {
            Ok(m) => m,
            Err(ref e)
                if e.kind() == io::ErrorKind::TimedOut || e.kind() == io::ErrorKind::WouldBlock =>
            {
                return Ok(false);
            }
            Err(e) => return Err(e),
        };

        if msg_type != protocol::MSG_REQUEST {
            return Ok(true);
        }

        let Ok(req) = bitcode::decode::<SessionServerRequest>(&payload) else {
            return Ok(true);
        };

        match req {
            SessionServerRequest::Write { id: _, data } => {
                if let Some(session) = self.session.as_mut() {
                    let _ = session.pty.write_bytes(&data);
                }
            }
            SessionServerRequest::Resize { id: _, cols, rows } => {
                if let Some(session) = self.session.as_mut() {
                    let size = portable_pty::PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    };
                    let _ = session.pty.resize(size);
                    session.parser.screen_mut().set_size(rows, cols);
                }
                let payload = bitcode::encode(&SessionServerResponse::Ok { id: None });
                protocol::send_msg(client, protocol::MSG_RESPONSE, &payload)?;
            }
            _ => {}
        }

        Ok(true)
    }

    fn push_session_updates(&mut self, client: &mut TcpStream) -> io::Result<bool> {
        let Some(session) = self.session.as_mut() else {
            return Ok(false);
        };
        let mut had_output = false;

        let bytes = session.read_output();
        if !bytes.is_empty() {
            had_output = true;
            let push = SessionServerPush::RawOutput { id: 1, data: bytes };
            let payload = bitcode::encode(&push);
            protocol::send_msg(client, protocol::MSG_PUSH, &payload)?;
        }

        if session.check_exited() {
            let status = session
                .pty
                .exit_status()
                .map_or(-1, |s| s.exit_code() as i32);
            if self.exit_code == 0 {
                self.exit_code = status;
            }
            let push = SessionServerPush::SessionExited { id: 1, status };
            let payload = bitcode::encode(&push);
            protocol::send_msg(client, protocol::MSG_PUSH, &payload)?;
        }

        if let Some(ref title) = session.title.take() {
            let push = SessionServerPush::TitleChanged {
                id: 1,
                title: title.clone(),
            };
            let payload = bitcode::encode(&push);
            protocol::send_msg(client, protocol::MSG_PUSH, &payload)?;
        }

        Ok(had_output)
    }

    fn send_welcome(&mut self, client: &mut TcpStream) -> io::Result<()> {
        let sessions = vec![(
            1,
            String::new(),
            self.session.as_ref().is_none_or(|s| s.exited),
        )];
        let push = SessionServerPush::Welcome { sessions };
        let payload = bitcode::encode(&push);
        protocol::send_msg(client, protocol::MSG_PUSH, &payload)
    }

    fn send_snapshot(&mut self, client: &mut TcpStream) -> io::Result<()> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        let data = session.generate_snapshot();
        if !data.is_empty() {
            let push = SessionServerPush::Snapshot { id: 1, data };
            let payload = bitcode::encode(&push);
            protocol::send_msg(client, protocol::MSG_PUSH, &payload)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(cmd: Vec<String>) -> SessionServerConfig {
        SessionServerConfig {
            bind_addr: "127.0.0.1:0".into(),
            cmd,
            cols: 80,
            rows: 24,
        }
    }

    /// Build a command that exits immediately with the given exit code.
    #[cfg(not(windows))]
    fn exit_cmd(code: i32) -> Vec<String> {
        if code == 0 {
            vec!["true".into()]
        } else {
            vec!["sh".into(), "-c".into(), format!("exit {code}")]
        }
    }

    #[cfg(windows)]
    fn exit_cmd(code: i32) -> Vec<String> {
        vec!["cmd".into(), "/c".into(), format!("exit {code}")]
    }

    #[test]
    fn server_exits_when_child_exits_cleanly() {
        let mut server = SessionServer::new(make_config(exit_cmd(0)));
        let result = server.run();
        assert!(result.is_ok());
        assert_eq!(server.exit_code(), 0);
    }

    #[test]
    fn server_exits_with_child_exit_code() {
        let mut server = SessionServer::new(make_config(exit_cmd(42)));
        let result = server.run();
        assert!(result.is_ok());
        assert_eq!(server.exit_code(), 42);
    }

    #[test]
    fn client_reconnects() {
        use std::io::Read;
        use std::net::TcpStream;

        // Find a free port.
        let temp = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = temp.local_addr().unwrap().port();
        let addr = format!("127.0.0.1:{port}");
        drop(temp);

        // Run the server with a command that stays alive long enough.
        let mut server = SessionServer::new(SessionServerConfig {
            bind_addr: addr.clone(),
            cmd: vec![], // default shell — stays alive until client sends input
            cols: 80,
            rows: 24,
        });
        let handle = std::thread::spawn(move || {
            if let Err(e) = server.run() {
                panic!("server run failed: {e}");
            }
        });

        fn wait_for_first(
            addr: &str,
            handle: &std::thread::JoinHandle<impl std::fmt::Debug>,
            timeout: Duration,
        ) -> TcpStream {
            let deadline = std::time::Instant::now() + timeout;
            loop {
                if handle.is_finished() {
                    panic!("server thread exited before accepting connection");
                }
                match TcpStream::connect(addr) {
                    Ok(stream) => return stream,
                    Err(_) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) => panic!("timed out waiting for {addr}: {e}"),
                }
            }
        }

        // First connection.
        let mut c1 = wait_for_first(&addr, &handle, Duration::from_secs(8));
        c1.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut buf = [0u8; 1];
        c1.read_exact(&mut buf).unwrap();
        // Wait for the server to finish sending before we close.
        std::thread::sleep(Duration::from_millis(100));
        drop(c1);

        // Second connection: server should have processed disconnect.
        std::thread::sleep(Duration::from_millis(200));
        let mut c2 = wait_for_first(&addr, &handle, Duration::from_secs(8));
        c2.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut buf = [0u8; 1];
        c2.read_exact(&mut buf).unwrap();
        drop(c2);

        assert!(!handle.is_finished(), "server died after both connections");
    }

    #[test]
    fn second_connection_refused() {
        use std::io::Read;
        use std::net::TcpStream;

        // Find a free port by binding a temporary listener.
        let temp = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = temp.local_addr().unwrap().port();
        let addr = format!("127.0.0.1:{port}");
        drop(temp);

        let mut server = SessionServer::new(SessionServerConfig {
            bind_addr: addr.clone(),
            cmd: vec!["/bin/sleep".into(), "999".into()],
            cols: 80,
            rows: 24,
        });

        let handle = std::thread::spawn(move || server.run());

        fn wait_for_stream(
            addr: &str,
            handle: &std::thread::JoinHandle<impl std::fmt::Debug>,
            timeout: Duration,
        ) -> TcpStream {
            let deadline = std::time::Instant::now() + timeout;
            loop {
                if handle.is_finished() {
                    panic!("server thread exited before accepting connection");
                }
                match TcpStream::connect(addr) {
                    Ok(stream) => return stream,
                    Err(_) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) => panic!("timed out waiting for {addr}: {e}"),
                }
            }
        }

        let _c1 = wait_for_stream(&addr, &handle, Duration::from_secs(10));

        // Give the server time to accept the first client and set
        // client_connected = true.
        std::thread::sleep(Duration::from_millis(100));

        // A second client can connect (TCP handshake completes), but
        // the server immediately drops the connection without sending
        // any data.  Reading from it should return an error.
        let mut c2 = TcpStream::connect(&addr).unwrap();
        c2.set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let mut buf = [0u8; 8];
        let err = c2.read(&mut buf).unwrap_err();
        assert!(
            err.kind() == io::ErrorKind::ConnectionReset
                || err.kind() == io::ErrorKind::TimedOut
                || err.kind() == io::ErrorKind::WouldBlock
                || err.kind() == io::ErrorKind::UnexpectedEof,
            "expected connection reset/timeout/eof, got {err:?}"
        );
    }
}
