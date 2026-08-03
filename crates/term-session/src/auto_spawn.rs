use std::io;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use term_session_muxio_service_definitions::{
    ChannelName, gateway_channel_name, probe_ipc_endpoint,
};

/// Resolve the gateway channel name to probe/spawn.
/// Uses the runtime `TERM_WM_GATEWAY` override if present, else the static
/// user-scoped default (`term-wm/<user>/gateway`).
pub fn resolve_gateway() -> ChannelName {
    gateway_channel_name()
}

fn spawn_detached_server(bin: &std::path::Path) -> io::Result<Child> {
    let mut cmd = Command::new(bin);
    cmd.arg("--daemon");
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
        // Detached process: no console, so the parent console's CTRL_CLOSE_EVENT
        // never reaches the daemon. CREATE_NO_WINDOW is ignored when
        // DETACHED_PROCESS is set, so it is not passed. CREATE_NO_INHERIT
        // (in place of the unstable Command::inherit_handles, rust-lang
        // issue #146407) prevents the daemon from inheriting any caller
        // handles. Combined with Stdio::null above, wrappers/CI/SSH runners
        // never wait on inherited pipes.
        cmd.creation_flags(
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_INHERIT,
        );
    }
    cmd.spawn()
}

/// Windows creation flags (named per AGENTS.md magic-strings rule).
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
/// Detach the child from the parent's console entirely.
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x00000008;
/// Do not inherit the parent's handles (bInheritHandles=false).
#[cfg(windows)]
const CREATE_NO_INHERIT: u32 = 0x00800000;

/// Wait for the gateway to become reachable, spawning a detached daemon if
/// none is running.
///
/// Returns the gateway channel name string, which the caller passes to the
/// muxio IPC client. `bin` defaults to the current executable so tests can
/// point it at `CARGO_BIN_EXE_term-session`.
pub fn connect_or_spawn_server(bin: Option<&std::path::Path>) -> io::Result<String> {
    let gateway = resolve_gateway();
    let socket_name = gateway.to_string();

    if probe_ipc_endpoint(&gateway) {
        return Ok(socket_name);
    }

    let bin = bin
        .map(|b| b.to_path_buf())
        .unwrap_or_else(|| std::env::current_exe().expect("current exe path"));
    let mut child = spawn_detached_server(&bin)?;
    let start = Instant::now();
    let timeout = Duration::from_secs(3);
    let poll_interval = Duration::from_millis(50);

    while start.elapsed() < timeout {
        if probe_ipc_endpoint(&gateway) {
            return Ok(socket_name);
        }
        if let Ok(Some(status)) = child.try_wait() {
            // The spawned daemon died before the socket came up. Another racer
            // may have won the bind; re-probe before surfacing the failure.
            if probe_ipc_endpoint(&gateway) {
                return Ok(socket_name);
            }
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("Gateway exited during startup with status: {status}"),
            ));
        }
        thread::sleep(poll_interval);
    }

    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("Timed out waiting for gateway on channel '{gateway}'"),
    ))
}
