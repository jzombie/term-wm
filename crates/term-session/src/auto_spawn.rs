use std::io;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use term_session_muxio_service_definitions::{ChannelName, probe_ipc_endpoint};

/// Parameters forwarded to the auto-spawned server process.
#[derive(Clone, Debug)]
pub struct ServerSpawnConfig<'a> {
    pub channel: &'a ChannelName,
    pub cmd: &'a [String],
}

fn spawn_detached_server(cfg: &ServerSpawnConfig<'_>) -> io::Result<Child> {
    let bin = std::env::current_exe()?;
    let mut cmd = Command::new(bin);
    cmd.arg("--server")
        .arg("--channel")
        .arg(cfg.channel.to_string());
    if !cfg.cmd.is_empty() {
        cmd.arg("--").args(cfg.cmd);
    }
    // All stdio is detached: a daemon must not rely on the parent reading its
    // pipes. In particular, a piped stderr that is never drained lets the OS
    // pipe buffer fill, blocking the server's stderr writes and deadlocking
    // startup on every platform. Discard it instead.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Start the server in its own session and process group via setsid().
        // This is the only process-group manipulation done here: a child that
        // already became a process-group leader (e.g. via setpgid) would have
        // setsid() fail with EPERM. Detaching from the launching terminal means
        // the daemon can never freeze its input, and terminal Ctrl+C / Ctrl+Z /
        // SIGHUP-on-close are never delivered to it.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd.spawn()
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
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("Session server exited during startup with status: {status}"),
            ));
        }
        thread::sleep(poll_interval);
    }

    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("Timed out waiting for server on channel '{channel}'"),
    ))
}
