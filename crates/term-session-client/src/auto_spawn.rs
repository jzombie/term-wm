use std::io::{self, Read};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use term_session_muxio_service_definitions::{ChannelName, probe_ipc_endpoint};

/// Parameters forwarded to the auto-spawned server process.
#[derive(Clone, Debug)]
pub struct ServerSpawnConfig<'a> {
    pub channel: &'a ChannelName,
    pub cols: u16,
    pub rows: u16,
    pub cmd: &'a [String],
}

fn spawn_detached_server(cfg: &ServerSpawnConfig<'_>) -> io::Result<Child> {
    let bin = std::env::current_exe()?;
    let mut cmd = Command::new(bin);
    cmd.arg("--server").arg("--channel").arg(cfg.channel.to_string());
    cmd.arg("--cols").arg(cfg.cols.to_string());
    cmd.arg("--rows").arg(cfg.rows.to_string());
    if !cfg.cmd.is_empty() {
        cmd.arg("--").args(cfg.cmd);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        // Keep stderr so startup failures (PTY spawn, socket bind) can be surfaced.
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd.spawn()
}

/// Read the dead server's captured stderr so the real failure reason is reported.
fn child_stderr(child: &mut Child) -> String {
    let Some(mut stderr) = child.stderr.take() else {
        return String::new();
    };
    let mut buf = String::new();
    let _ = stderr.read_to_string(&mut buf);
    buf
}

/// Wait for a session server to become reachable on the channel, spawning one
/// via `current_exe() --server` if none is running.
///
/// Returns the channel name string, which the caller passes to the muxio IPC
/// client. The client and server both route it through `GenericNamespaced`, so
/// no filesystem path is involved.
pub fn connect_or_spawn_server(
    channel: &ChannelName,
    cfg: &ServerSpawnConfig<'_>,
) -> io::Result<String> {
    let socket_name = channel.to_string();

    if probe_ipc_endpoint(channel) {
        return Ok(socket_name);
    }

    let mut child = spawn_detached_server(cfg)?;
    let start = Instant::now();
    let timeout = Duration::from_secs(3);
    let poll_interval = Duration::from_millis(50);

    while start.elapsed() < timeout {
        if probe_ipc_endpoint(channel) {
            return Ok(socket_name);
        }
        if let Ok(Some(status)) = child.try_wait() {
            // The spawned server died before the socket came up. Another racer
            // may have won the bind; re-probe before surfacing the failure.
            if probe_ipc_endpoint(channel) {
                return Ok(socket_name);
            }
            let stderr = child_stderr(&mut child);
            let detail = if stderr.trim().is_empty() {
                status.to_string()
            } else {
                format!("{status}: {}", stderr.trim())
            };
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("Session server exited during startup ({detail})"),
            ));
        }
        thread::sleep(poll_interval);
    }

    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("Timed out waiting for server on channel '{channel}'"),
    ))
}
