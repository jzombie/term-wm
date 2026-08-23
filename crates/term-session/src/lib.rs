#![doc = include_str!("../README.md")]

pub mod auto_spawn;

pub use muxio_tokio_rpc_ipc_client as rpc_client;
pub use term_session_client as client;
pub use term_session_muxio_service_definitions as protocol;
pub use term_session_muxio_service_definitions::{
    ChannelName, DEFAULT_WORKSPACE, SESSION_CHANNEL_NAME, gateway_channel_name, gateway_help_line,
};
pub use term_session_server as server;

use std::io;
use std::sync::Arc;

use muxio_tokio_rpc_ipc_client::RpcCallPrebuffered;
use term_session_muxio_service_definitions::{
    KillChannel, KillClient, ListChannels, ListChannelsResponse, ListWmStats, ShutdownGateway,
    WmStatsEntry,
};

/// Run a CLI entry point and report any error identically across every
/// term-wm-family binary, then terminate with the matching exit code.
///
/// On success this exits `0`. On error it prints `error: {err}` (the `Display`
/// form — not Rust's default `Error: {err:?}` debug dump) to the *original*
/// stderr and exits `1`. The original stderr is preserved because the TUI
/// client may `dup2` fd 2 into the tracing pipe (see `redirect_fd_to_tracing`);
/// a fatal error surfaced after the UI has started must still reach the user.
///
/// `term-session` and `term-wm` both route their `main()` through this so
/// fatal-error formatting and exit codes stay uniform.
pub fn run_and_exit(run: impl FnOnce() -> io::Result<()>) -> ! {
    // Preserve the real stderr before the entry point may redirect fd 2 into
    // the tracing pipe. Best-effort: if `dup` fails, fall back to `eprintln!`.
    #[cfg(unix)]
    let orig_stderr: Option<std::fs::File> = {
        use std::os::unix::io::FromRawFd;
        let fd = unsafe { libc::dup(libc::STDERR_FILENO) };
        if fd >= 0 {
            Some(unsafe { std::fs::File::from_raw_fd(fd) })
        } else {
            None
        }
    };

    if let Err(err) = run() {
        #[cfg(unix)]
        {
            use std::io::Write;
            match orig_stderr {
                Some(mut f) => {
                    let _ = writeln!(f, "error: {err}");
                }
                None => eprintln!("error: {err}"),
            }
        }
        #[cfg(not(unix))]
        eprintln!("error: {err}");
        std::process::exit(1);
    }
    std::process::exit(0);
}

pub use term_wm_config::env::CHANNEL_ENV_VAR;
pub const DEFAULT_CHANNEL: &str = "default/main";

/// Resolve the channel from an optional CLI arg, falling back to the env var,
/// then the default.
pub fn resolve_channel(cli_channel: Option<String>) -> String {
    cli_channel
        .or_else(|| std::env::var(CHANNEL_ENV_VAR).ok())
        .unwrap_or_else(|| DEFAULT_CHANNEL.to_string())
}

/// Seconds per minute, used by [`format_unix_relative`].
const SECS_PER_MIN: u64 = 60;
/// Seconds per hour, used by [`format_unix_relative`].
const SECS_PER_HOUR: u64 = 3600;
/// Seconds per day, used by [`format_unix_relative`].
const SECS_PER_DAY: u64 = 86400;

/// Format a unix timestamp as a relative human string ("2s ago", "5m ago", …),
/// always in elapsed units regardless of age ("2d 5h" for ages beyond a day).
pub fn format_unix_relative(ts: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_unix_relative_at(ts, now)
}

/// Format a unix timestamp relative to an explicit `now` in unix seconds.
///
/// Elapsed durations are always rendered in relative units: seconds, minutes,
/// hours, then combined days + hours. A zero timestamp renders as `-`.
/// Timestamps newer than `now` saturate to the seconds tier.
pub fn format_unix_relative_at(ts: u64, now: u64) -> String {
    if ts == 0 {
        return "-".to_string();
    }
    let diff = now.saturating_sub(ts);
    if diff < SECS_PER_MIN {
        format!("{diff}s")
    } else if diff < SECS_PER_HOUR {
        format!("{}m", diff / SECS_PER_MIN)
    } else if diff < SECS_PER_DAY {
        format!("{}h", diff / SECS_PER_HOUR)
    } else {
        format!(
            "{}d {}h",
            diff / SECS_PER_DAY,
            (diff % SECS_PER_DAY) / SECS_PER_HOUR
        )
    }
}

/// Upper bound for ONE gateway IPC exchange (connect + call combined).
/// Generous for slow CI and cold daemon starts, but finite: an unresponsive
/// gateway must fail loudly instead of parking the caller forever. On expiry
/// the worker thread is abandoned (the established in-repo pattern from the
/// `*_bounded` helpers); its handles die with the process and every test
/// gateway name is pid/counter-unique, so no cross-test pipe contention
/// results.
const GATEWAY_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Connect to the gateway daemon and run `op` with a live client. Spawns an
/// OS thread so the new Tokio runtime is fully isolated from any runtime
/// already active on the calling thread (avoids the `block_on`-inside-runtime
/// panic). Bounded by [`GATEWAY_CALL_TIMEOUT`]; see
/// [`with_gateway_timeout`] for the deadline semantics.
pub fn with_gateway<F, Fut, T>(op: F) -> io::Result<T>
where
    F: FnOnce(Arc<muxio_tokio_rpc_ipc_client::RpcIpcClient>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    with_gateway_timeout(GATEWAY_CALL_TIMEOUT, op)
}

/// Like [`with_gateway`], but every phase (connect, then `op`) is bounded by
/// TWO stacked defenses:
///
/// 1. Cooperative `tokio::time::timeout` around each future (muxio's client
///    uses interprocess's tokio transport, so connects are cancellable).
/// 2. A hard OS-level bound: the worker reports through a channel and the
///    caller waits via `recv_timeout(total_timeout)`, so even a worker stuck
///    in a non-cooperative kernel call can only delay the caller by
///    `total_timeout` before it returns `TimedOut`. The abandoned worker's
///    handles are reclaimed at process exit.
///
/// Ops keep their existing output type (typically the inner service Result);
/// no call-site changes are needed.
pub fn with_gateway_timeout<F, Fut, T>(timeout: std::time::Duration, op: F) -> io::Result<T>
where
    F: FnOnce(Arc<muxio_tokio_rpc_ipc_client::RpcIpcClient>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let gateway = term_session_muxio_service_definitions::gateway_channel_name();
    let (tx, rx) = std::sync::mpsc::channel();
    let total_timeout = timeout.saturating_mul(2);
    let gateway_display = gateway.to_string();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        let result = match runtime {
            Err(e) => Err(io::Error::other(format!("runtime: {e}"))),
            Ok(rt) => rt.block_on(async {
                // RpcIpcClient::new already returns Arc<RpcIpcClient>, which
                // satisfies op's parameter without another wrapper.
                let client =
                    tokio::time::timeout(timeout, muxio_tokio_rpc_ipc_client::RpcIpcClient::new(
                        &gateway.to_string(),
                    ))
                    .await
                    .map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("gateway connect timed out after {timeout:?}"),
                        )
                    })?
                    .map_err(|e| {
                        io::Error::new(
                            io::ErrorKind::ConnectionRefused,
                            format!(
                                "No gateway daemon is running on '{gateway}'. Start one with `term-session --channel <name>` or `term-session --daemon` first.\n  cause: {e}"
                            ),
                        )
                    })?;
                let out = tokio::time::timeout(timeout, op(client))
                    .await
                    .map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("gateway call timed out after {timeout:?}"),
                        )
                    })?;
                Ok(out)
            }),
        };
        let _ = tx.send(result);
    });
    match rx.recv_timeout(total_timeout) {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("gateway exchange timed out after {total_timeout:?} on '{gateway_display}'"),
        )),
    }
}

/// List channels from the gateway, including the daemon PID + socket name.
pub fn list_channels() -> io::Result<ListChannelsResponse> {
    with_gateway(|client| async move { ListChannels::call(&*client, ()).await })?
        .map_err(|e| io::Error::other(format!("list: {e}")))
}

/// Snapshot every channel's aggregated WM stats. Channels without a
/// reporting connection are omitted (unknown, not zero). Bounded like
/// [`list_channels_bounded`]: on timeout the worker thread is abandoned and
/// `io::ErrorKind::TimedOut` is returned.
///
/// Note: stats are WRITTEN by each inner WM over its subscribed internal-WM
/// connection (see `main.rs`'s attributed-input listener), not through this
/// ephemeral-connection helper family; a fresh connection cannot prove which
/// channel it belongs to.
pub fn list_wm_stats_bounded(timeout: std::time::Duration) -> io::Result<Vec<WmStatsEntry>> {
    let gateway = term_session_muxio_service_definitions::gateway_channel_name();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| io::Error::other(format!("runtime: {e}")))?;
            rt.block_on(async {
                let client = muxio_tokio_rpc_ipc_client::RpcIpcClient::new(&gateway.to_string())
                    .await
                    .map_err(|e| {
                        io::Error::new(io::ErrorKind::ConnectionRefused, format!("connect: {e}"))
                    })?;
                ListWmStats::call(&*client, ())
                    .await
                    .map(|resp| resp.stats)
                    .map_err(|e| io::Error::other(format!("list_wm_stats: {e}")))
            })
        })();
        let _ = tx.send(result);
    });
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "wm stats listing timed out",
        )),
    }
}

/// Like [`list_channels`], but bounds the total wait (connect + round trip).
///
/// Used by UI code that must never hang on an unresponsive daemon socket. On
/// timeout the worker thread is abandoned — it terminates on its own once its
/// pending socket I/O completes — and `io::ErrorKind::TimedOut` is returned.
pub fn list_channels_bounded(timeout: std::time::Duration) -> io::Result<ListChannelsResponse> {
    let gateway = term_session_muxio_service_definitions::gateway_channel_name();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| io::Error::other(format!("runtime: {e}")))?;
            rt.block_on(async {
                let client = muxio_tokio_rpc_ipc_client::RpcIpcClient::new(&gateway.to_string())
                    .await
                    .map_err(|e| {
                        io::Error::new(io::ErrorKind::ConnectionRefused, format!("connect: {e}"))
                    })?;
                ListChannels::call(&*client, ())
                    .await
                    .map_err(|e| io::Error::other(format!("list: {e}")))
            })
        })();
        let _ = tx.send(result);
    });
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "gateway channel listing timed out",
        )),
    }
}

/// Print a human-readable listing of all channels to stdout, including the
/// gateway daemon PID + socket. Returns the same error as [`list_channels`]
/// if the gateway is unreachable.
pub fn print_list() -> io::Result<()> {
    let resp = list_channels()?;
    // Header: which PID on this system is the gateway daemon.
    println!(
        "Gateway Daemon PID: {} | Socket: {}",
        resp.gateway_pid, resp.socket
    );
    if resp.channels.is_empty() {
        println!("\nNo channels.");
        return Ok(());
    }
    // Vertical list: one block per channel, one line per client. Kept short so
    // it wraps cleanly instead of being a wide table.
    for ch in &resp.channels {
        let session = ch
            .session
            .as_ref()
            .map(|s| format!("shared size: {}x{}", s.cols, s.rows))
            .unwrap_or_else(|| "none".to_string());
        let nclients = ch.clients.len();
        println!();
        println!("channel: {}", ch.name);
        println!("  created: {}", format_unix_relative(ch.created_at_unix));
        println!("  {session}");
        println!(
            "  clients: {}",
            if nclients == 0 {
                "none".to_string()
            } else {
                format!("{nclients} connected")
            }
        );
        for c in &ch.clients {
            println!("    - conn: {}  (pid {})", c.conn_id, c.pid);
            println!("      user: {}", c.user);
            println!("      version: {}", c.version);
            if let Some(ip) = &c.ssh_ip {
                println!("      ssh ip from: {}", ip);
            }
            println!("      host: {}", c.hostname);
            println!("      size: {}x{}", c.cols, c.rows);
            println!(
                "      connected: {}",
                format_unix_relative(c.connected_at_unix)
            );
        }
    }
    Ok(())
}

/// Kill a channel's session and detach all its sockets.
///
/// The gateway refuses while any participant is attached to the channel unless
/// `force` is true (see `RPC_ERROR_LIVE_PARTICIPANTS`).
pub fn kill_channel(channel: &str, force: bool) -> io::Result<()> {
    let ch = channel.to_string();
    with_gateway(move |client| async move { KillChannel::call(&*client, (ch, force)).await })?
        .map_err(|e| io::Error::other(format!("kill channel: {e}")))
}

/// Detach a single client socket from a channel by `conn_id`.
pub fn kill_client(channel: &str, conn_id: usize) -> io::Result<()> {
    let ch = channel.to_string();
    with_gateway(move |client| async move { KillClient::call(&*client, (ch, conn_id)).await })?
        .map_err(|e| io::Error::other(format!("kill client: {e}")))
}

/// Request the gateway to rebind viewers attached to `source_channel`
/// over to the `target` workspace (CallerOnly by default, falls back to single viewer if initiator is None).
pub fn request_workspace_rebind(source_channel: &str, target: &str) -> io::Result<()> {
    request_workspace_rebind_with_scope(
        source_channel,
        target,
        term_session_muxio_service_definitions::RebindScope::CallerOnly,
        None,
    )
}

/// Request rebind with explicit scope and optional initiator.
pub fn request_workspace_rebind_with_scope(
    source_channel: &str,
    target: &str,
    scope: term_session_muxio_service_definitions::RebindScope,
    initiator_conn_id: Option<usize>,
) -> io::Result<()> {
    let source_owned = source_channel.to_string();
    let target_owned = target.to_string();
    with_gateway(move |client| async move {
        use muxio_tokio_rpc_ipc_client::RpcCallPrebuffered;
        use term_session_muxio_service_definitions::{RebindWorkspace, RebindWorkspaceRequest};

        RebindWorkspace::call(
            &*client,
            RebindWorkspaceRequest {
                source_channel: source_owned,
                target: target_owned,
                scope,
                initiator_conn_id,
            },
        )
        .await
    })?
    .map_err(|e| io::Error::other(format!("rebind workspace: {e}")))
}

/// List connected users on a channel.
pub fn list_users(
    channel: &str,
) -> io::Result<term_session_muxio_service_definitions::ListUsersResponse> {
    let ch = channel.to_string();
    with_gateway(move |client| async move {
        use term_session_muxio_service_definitions::ListUsers;
        ListUsers::call(&*client, ch).await
    })?
    .map_err(|e| io::Error::other(format!("list users: {e}")))
}

/// Stop the gateway daemon.
///
/// The daemon refuses to shut down while any live session is running unless
/// `force` is true (see `RPC_ERROR_LIVE_SESSIONS`).
pub fn stop_gateway(force: bool) -> io::Result<()> {
    with_gateway(move |client| async move { ShutdownGateway::call(&*client, force).await })?
        .map_err(|e| io::Error::other(format!("shutdown: {e}")))
}

/// Run the gateway daemon: rename the process, detach from the controlling
/// terminal, and serve until `ShutdownGateway`. `selfcheck_marker` is a
/// test-only path written with the platform's detachment proof once bound.
pub fn run_daemon(selfcheck_marker: Option<std::path::PathBuf>) -> io::Result<()> {
    tracing_subscriber::fmt::init();

    // Make the daemon recognizable in process managers: every `term-session`
    // process is the same binary, so rename this one so `ps`/`top`/Task
    // Manager show `term-session-daemon` instead of generic `term-session`.
    set_daemon_process_name();

    // Self-detach: a `--daemon` that was not already started detached (e.g.
    // spawned directly by a test or wrapper, not via
    // `auto_spawn::connect_or_spawn_server`) detaches itself from the
    // launching terminal so Ctrl+C / SIGHUP never reach it.
    //
    // - Unix: `setsid()` starts a new session and process group and drops the
    //   controlling terminal. It fails with EPERM if the process is already a
    //   process-group leader, which is exactly the already-detached case — so
    //   ignore that error.
    // - Windows: `FreeConsole()` detaches from the launching console so no
    //   console control events (Ctrl+C, Ctrl+Close) are ever delivered to the
    //   daemon. It reports failure when there is no console to detach from,
    //   which is the already-detached `auto_spawn` case — so ignore that too.
    #[cfg(unix)]
    unsafe {
        libc::setsid();
    }
    #[cfg(windows)]
    unsafe {
        let _ = windows_sys::Win32::System::Console::FreeConsole();
    }

    let gateway = term_session_muxio_service_definitions::gateway_channel_name();

    // Test-only: as soon as the gateway socket is reachable, write the
    // platform's detachment proof to the marker, then exit the probe thread.
    if let Some(ref marker) = selfcheck_marker {
        let gw = gateway.clone();
        let marker = marker.clone();
        std::thread::Builder::new()
            .name("daemon-selfcheck".into())
            .spawn(move || {
                for _ in 0..200 {
                    if term_session_muxio_service_definitions::probe_ipc_endpoint(&gw) {
                        write_selfcheck_marker(&marker);
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                let _ = std::fs::write(&marker, "bound-timeout");
            })?;
    }

    let rt =
        tokio::runtime::Runtime::new().map_err(|e| io::Error::other(format!("runtime: {e}")))?;
    rt.block_on(term_session_server::run_gateway(gateway.clone()))
        .map_err(|e| io::Error::other(format!("gateway error: {e}")))?;
    Ok(())
}

/// Rename the running process so process managers can distinguish the gateway
/// daemon from interactive `term-session` clients. Best-effort and cosmetic:
/// a failure is ignored and never affects functionality.
///
/// Platform behavior (and limitations):
/// - **Linux:** `PR_SET_NAME` sets the process comm (capped at 15 bytes →
///   `term-session-d`), so `ps -comm`, `top`, and `htop` show the renamed
///   value. This is the most complete rename on any platform.
/// - **macOS:** `pthread_setname_np` sets the **thread** name, not the process
///   comm — `ps -o comm` and Activity Monitor's process list still show
///   `term-session`. The renamed value is only visible in Activity Monitor's
///   per-thread view (and `sample`). This is an OS limitation: macOS has no
///   portable user-space API to rename the process comm. Daemon disambiguation
///   on macOS therefore relies primarily on the `--daemon` argv flag and the
///   `Gateway Daemon PID` header printed by `term-session list`.
/// - **Windows:** `SetThreadDescription` sets the thread description, which
///   Process Explorer / Process Hacker show in the **Description** column.
pub fn set_daemon_process_name() {
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        if let Ok(name) = CString::new("term-session-d") {
            unsafe {
                libc::prctl(libc::PR_SET_NAME, name.as_ptr() as usize, 0, 0, 0);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        if let Ok(name) = CString::new("term-session-daemon") {
            unsafe {
                libc::pthread_setname_np(name.as_ptr());
            }
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadDescription};
        let wide: Vec<u16> = "term-session-daemon"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            SetThreadDescription(GetCurrentThread(), wide.as_ptr());
        }
    }
}

/// Write the platform's detachment proof to the marker (test-only).
fn write_selfcheck_marker(marker: &std::path::Path) {
    #[cfg(windows)]
    let proof = {
        use windows_sys::Win32::System::Console::{
            GetConsoleProcessList, GetStdHandle, STD_INPUT_HANDLE,
        };
        let mut pids = [0u32; 4];
        let count = unsafe {
            let _handle = GetStdHandle(STD_INPUT_HANDLE);
            GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32)
        };
        if count == 0 {
            "windows-no-console"
        } else {
            "windows-has-console"
        }
    };
    #[cfg(unix)]
    let proof = {
        let sid = unsafe { libc::getsid(0) };
        let pid = unsafe { libc::getpid() };
        if sid == pid {
            "unix-session-leader"
        } else {
            "unix-not-leader"
        }
    };
    #[cfg(not(any(unix, windows)))]
    let proof = "unsupported";
    let _ = std::fs::write(marker, proof);
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate `TERM_SESSION_CHANNEL`, which is process-global.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn cli_channel_takes_precedence_over_env() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var(CHANNEL_ENV_VAR, "other/chan");
        }
        assert_eq!(resolve_channel(Some("work/dev".to_string())), "work/dev");
        unsafe {
            std::env::remove_var(CHANNEL_ENV_VAR);
        }
    }

    #[test]
    fn falls_back_to_env_channel() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var(CHANNEL_ENV_VAR, "work/dev");
        }
        assert_eq!(resolve_channel(None), "work/dev");
        unsafe {
            std::env::remove_var(CHANNEL_ENV_VAR);
        }
    }

    #[test]
    fn falls_back_to_default_channel() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var(CHANNEL_ENV_VAR);
        }
        assert_eq!(resolve_channel(None), DEFAULT_CHANNEL);
    }

    #[test]
    fn format_zero_timestamp_is_dash() {
        assert_eq!(format_unix_relative_at(0, SECS_PER_DAY), "-");
    }

    #[test]
    fn format_under_a_minute_shows_seconds() {
        assert_eq!(
            format_unix_relative_at(SECS_PER_DAY - 42, SECS_PER_DAY),
            "42s"
        );
    }

    #[test]
    fn format_under_an_hour_shows_minutes() {
        assert_eq!(
            format_unix_relative_at(SECS_PER_DAY - 3_300, SECS_PER_DAY),
            "55m"
        );
    }

    #[test]
    fn format_under_a_day_shows_hours() {
        assert_eq!(
            format_unix_relative_at(SECS_PER_DAY - 7_200, SECS_PER_DAY),
            "2h"
        );
    }

    #[test]
    fn format_older_than_a_day_shows_days_and_hours() {
        assert_eq!(
            format_unix_relative_at(10 * SECS_PER_DAY, 11 * SECS_PER_DAY),
            "1d 0h"
        );
        assert_eq!(
            format_unix_relative_at(10 * SECS_PER_DAY, 11 * SECS_PER_DAY + 3 * SECS_PER_HOUR),
            "1d 3h"
        );
    }

    #[test]
    fn format_day_boundary_exact() {
        assert_eq!(
            format_unix_relative_at(10 * SECS_PER_DAY, 11 * SECS_PER_DAY),
            "1d 0h"
        );
    }

    #[test]
    fn format_timestamp_newer_than_now_saturates() {
        assert_eq!(
            format_unix_relative_at(SECS_PER_DAY + 10, SECS_PER_DAY),
            "0s"
        );
    }

    #[test]
    fn format_does_not_render_clock_time() {
        // Regression for the military-time leak: an old timestamp rendered
        // `ts % 86400` (UTC time-of-day). It must never produce HH:MM:SS.
        let ts = SECS_PER_DAY * 40 + 18 * SECS_PER_HOUR + 48 * SECS_PER_MIN + 46;
        let out = format_unix_relative_at(ts, SECS_PER_DAY * 42);
        assert_eq!(out, "1d 5h");
        assert!(!out.contains(':'), "clock-time format leaked: {out}");
    }
}
