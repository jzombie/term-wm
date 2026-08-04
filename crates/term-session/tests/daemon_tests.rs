//! Binary/daemon tests for the `term-session` gateway.
//!
//! These exercise the real compiled binary (`CARGO_BIN_EXE_term-session`):
//! detachment proof via `--daemon-selfcheck`, daemon resilience to client
//! disconnects and parent death, and clean teardown via `ShutdownGateway`.
//!
//! Each test uses a unique `TERM_WM_GATEWAY` so parallel runs never collide.

use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use muxio_tokio_rpc_ipc_client::RpcCallPrebuffered;
use term_session_muxio_service_definitions::{
    Attach, AttachRequest, ChannelName, ListChannels, ShutdownGateway, Spawn, probe_ipc_endpoint,
};

/// The compiled `term-session` binary under test.
fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_term-session"))
}

/// Path to the mock PTY binary used as a session child. Delegates to the
/// shared helper in the mock crate's library.
fn mock_bin() -> PathBuf {
    term_session_mock::get_mock_bin()
}

/// A unique per-test gateway name.
fn unique_gateway(tag: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("term-wm/dtest-{tag}-{id}")
}

/// Spawn the real daemon with the given gateway and an optional selfcheck
/// marker. Returns `(child, marker_path)`.
fn spawn_daemon(gateway: &str, selfcheck: bool) -> (Child, Option<PathBuf>) {
    let marker = if selfcheck {
        let path = std::env::temp_dir().join(format!(
            "term-session-selfcheck-{}.txt",
            gateway.replace('/', "-")
        ));
        let _ = std::fs::remove_file(&path);
        Some(path)
    } else {
        None
    };
    let mut cmd = Command::new(bin());
    cmd.env("TERM_WM_GATEWAY", gateway).arg("--daemon");
    if let Some(ref m) = marker {
        cmd.arg("--daemon-selfcheck").arg(m);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = cmd.spawn().expect("spawn daemon");
    (child, marker)
}

/// Attach a client to a channel and return its server-assigned conn id,
/// using the mock identity fields the other helpers expect.
async fn attach_to(
    client: &muxio_tokio_rpc_ipc_client::RpcIpcClient,
    channel: &str,
    hostname: &str,
) -> usize {
    Attach::call(
        client,
        AttachRequest {
            channel: channel.to_string(),
            hostname: hostname.to_string(),
            pid: std::process::id() as u64,
            user: "test-user".to_string(),
            version: "test-version".to_string(),
            ssh_ip: None,
        },
    )
    .await
    .expect("attach")
}

/// Poll until a client can connect to the gateway, or panic after a timeout.
async fn wait_connectable(gateway: &str) -> Arc<muxio_tokio_rpc_ipc_client::RpcIpcClient> {
    let start = Instant::now();
    loop {
        match muxio_tokio_rpc_ipc_client::RpcIpcClient::new(gateway).await {
            Ok(c) => return c,
            Err(_) if start.elapsed() < Duration::from_secs(20) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("gateway {gateway} not reachable after 20s: {e}"),
        }
    }
}

#[tokio::test]
async fn daemon_detaches_and_reports_proof() {
    let gateway = unique_gateway("detach");
    let (mut child, marker) = spawn_daemon(&gateway, true);
    let marker = marker.expect("marker requested");

    // Wait for the marker (daemon writes it once bound).
    let start = Instant::now();
    let proof = loop {
        if let Ok(content) = std::fs::read_to_string(&marker) {
            break content.trim().to_string();
        }
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "daemon never wrote selfcheck marker"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // Platform-specific detachment proof.
    #[cfg(windows)]
    assert_eq!(proof, "windows-no-console", "marker: {proof}");
    #[cfg(unix)]
    assert_eq!(proof, "unix-session-leader", "marker: {proof}");

    // Clean up.
    let client = wait_connectable(&gateway).await;
    ShutdownGateway::call(&*client, true).await.unwrap();
    let _ = child.wait();
}

#[tokio::test]
async fn daemon_survives_all_clients_disconnecting() {
    let gateway = unique_gateway("survive");
    let (mut child, _marker) = spawn_daemon(&gateway, false);

    let client = wait_connectable(&gateway).await;
    let channel = "test/daemon_survive";
    attach_to(&client, channel, "t").await;
    Spawn::call(
        &*client,
        (
            Some(vec![
                mock_bin().to_string_lossy().to_string(),
                "sleep".into(),
                "60000".into(),
            ]),
            80u16,
            24u16,
        ),
    )
    .await
    .unwrap();
    drop(client);

    // After ALL clients disconnect, the daemon must still be reachable and a
    // fresh attach/spawn must succeed (session respawns / persists).
    let client2 = wait_connectable(&gateway).await;
    attach_to(&client2, channel, "t").await;
    Spawn::call(&*client2, (None, 80u16, 24u16)).await.unwrap();

    ShutdownGateway::call(&*client2, true).await.unwrap();
    let _ = child.wait();
}

#[tokio::test]
async fn daemon_survives_parent_death() {
    let gateway = unique_gateway("parent_death");
    let channel = "test/daemon_parent_death";
    let mock = mock_bin().to_string_lossy().to_string();

    // Spawn a client that auto-spawns the daemon, running a LONG-LIVED
    // session so its process survives the parent dying.
    let mut client = Command::new(bin())
        .env("TERM_WM_GATEWAY", &gateway)
        .args(["--channel", channel, "--", &mock, "sleep", "60000"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn auto-attach client");

    // Give it time to auto-spawn the daemon and attach.
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let _ = client.kill();
    let _ = client.wait();

    // The daemon it spawned must still be reachable and the session alive
    // (the `sleep` process is still running, so the daemon must not have
    // exited — sessions are torn down only when their process ends).
    let client = wait_connectable(&gateway).await;
    attach_to(&client, channel, "t").await;
    let (id, _, _) = Spawn::call(&*client, (None, 80u16, 24u16)).await.unwrap();
    assert_eq!(id, 1, "session from the orphaned daemon must persist");

    ShutdownGateway::call(&*client, true).await.unwrap();
    // Give the daemon time to run its teardown and exit.
    tokio::time::sleep(Duration::from_millis(1000)).await;
}

/// The daemon must never inherit the parent's open handles. Regression guard
/// for the Windows `bInheritHandles = FALSE` auto-spawn: `std::process::Command`
/// always passes `TRUE`, so a future switch back to it would leak every
/// inheritable handle (pipe ends, sockets) into the daemon.
///
/// The test creates an inheritable pipe, auto-spawns the daemon through the
/// real `connect_or_spawn_server` path, closes the parent's write end, and
/// asserts the read end reaches EOF. If the daemon inherited the write handle
/// it stays open forever and the assertion times out.
#[tokio::test]
async fn daemon_does_not_inherit_parent_handles() {
    use term_session::auto_spawn::connect_or_spawn_server;

    let gateway = unique_gateway("no_inherit");
    // `connect_or_spawn_server` resolves the gateway from `TERM_WM_GATEWAY` in
    // this process's environment; point it at the unique per-test channel.
    // `set_var` is `unsafe` under edition 2024.
    unsafe {
        std::env::set_var("TERM_WM_GATEWAY", &gateway);
    }

    #[cfg(windows)]
    let (read_end, write_end) = create_inheritable_pipe();
    #[cfg(unix)]
    let (read_end, write_end) = create_cloexec_pipe();
    #[cfg(not(any(unix, windows)))]
    panic!("handle-inheritance test not supported on this platform");

    // Auto-spawn the detached daemon via the real auto-spawn path.
    connect_or_spawn_server(Some(&bin())).expect("auto-spawn daemon");

    // Close the parent's write end. A correctly detached daemon holds no copy,
    // so the read end reaches EOF; a daemon that inherited the handle keeps the
    // pipe open indefinitely.
    close_write_end(write_end);

    assert_eof_on_read_end(read_end, Duration::from_secs(5))
        .expect("daemon inherited the parent's pipe write end");

    close_read_end(read_end);

    // Clean up the daemon.
    let client = wait_connectable(&gateway).await;
    ShutdownGateway::call(&*client, true).await.unwrap();
}

/// Create an inheritable named pipe whose read end we keep. Both ends are
/// marked inheritable via `SECURITY_ATTRIBUTES`, so if the daemon is spawned
/// with `bInheritHandles = TRUE` (the regression under test) it keeps the
/// write end and the pipe never breaks.
#[cfg(windows)]
fn create_inheritable_pipe() -> (
    windows_sys::Win32::Foundation::HANDLE,
    windows_sys::Win32::Foundation::HANDLE,
) {
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Pipes::CreatePipe;

    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let mut read = std::ptr::null_mut();
    let mut write = std::ptr::null_mut();
    let ok = unsafe { CreatePipe(&mut read, &mut write, &sa, 0) };
    assert_ne!(ok, 0, "CreatePipe failed: {}", io::Error::last_os_error());
    (read, write)
}

/// Create a pipe with both ends CLOEXEC. The std `Command` spawn path never
/// clears CLOEXEC, so a correctly detached daemon never holds the write end.
#[cfg(unix)]
fn create_cloexec_pipe() -> (libc::c_int, libc::c_int) {
    use std::os::unix::io::RawFd;

    let mut fds = [0 as RawFd; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe() failed");
    for &fd in &fds {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert!(flags >= 0, "F_GETFD failed");
        let rc = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
        assert_eq!(rc, 0, "F_SETFD failed");
    }
    (fds[0], fds[1])
}

/// Assert the read end reaches EOF (write end fully closed) within `timeout`.
#[cfg(windows)]
fn assert_eof_on_read_end(
    read: windows_sys::Win32::Foundation::HANDLE,
    timeout: Duration,
) -> io::Result<()> {
    use windows_sys::Win32::Foundation::ERROR_BROKEN_PIPE;
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    let start = Instant::now();
    loop {
        let mut total_avail: u32 = 0;
        let ok = unsafe {
            PeekNamedPipe(
                read,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut total_avail,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                // Every write-end handle is gone: the daemon inherited none.
                return Ok(());
            }
            return Err(err);
        }
        if start.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "read end never reached EOF (daemon inherited the write handle)",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Assert the read end reaches EOF (write end fully closed) within `timeout`.
#[cfg(unix)]
fn assert_eof_on_read_end(fd: libc::c_int, timeout: Duration) -> io::Result<()> {
    let start = Instant::now();
    loop {
        let mut poll_fds = [libc::pollfd {
            fd,
            events: libc::POLLIN | libc::POLLHUP,
            revents: 0,
        }];
        let n = unsafe { libc::poll(poll_fds.as_mut_ptr(), 1, 50) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n > 0 {
            // Drain any buffered bytes; EOF is a zero-length read.
            let mut buf = [0u8; 64];
            loop {
                let r = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                if r < 0 {
                    if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(io::Error::last_os_error());
                }
                if r == 0 {
                    return Ok(());
                }
                if (r as usize) < buf.len() {
                    break;
                }
            }
        }
        if start.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "read end never reached EOF (child inherited the write fd)",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(windows)]
fn close_read_end(read: windows_sys::Win32::Foundation::HANDLE) {
    unsafe {
        let _ = windows_sys::Win32::Foundation::CloseHandle(read);
    }
}

#[cfg(windows)]
fn close_write_end(write: windows_sys::Win32::Foundation::HANDLE) {
    unsafe {
        let _ = windows_sys::Win32::Foundation::CloseHandle(write);
    }
}

#[cfg(unix)]
fn close_read_end(read: libc::c_int) {
    unsafe {
        let _ = libc::close(read);
    }
}

#[cfg(unix)]
fn close_write_end(write: libc::c_int) {
    unsafe {
        let _ = libc::close(write);
    }
}

#[tokio::test]
async fn cli_kill_client_detaches_one_client() {
    let gateway = unique_gateway("kill_client");
    let channel = "test/kill_client";
    let (mut child, _marker) = spawn_daemon(&gateway, false);

    // Two attached clients on the same channel.
    let c1 = wait_connectable(&gateway).await;
    let c2 = wait_connectable(&gateway).await;
    attach_to(&c1, channel, "one").await;
    attach_to(&c2, channel, "two").await;
    Spawn::call(
        &*c1,
        (
            Some(vec![
                mock_bin().to_string_lossy().to_string(),
                "sleep".into(),
                "60000".into(),
            ]),
            80u16,
            24u16,
        ),
    )
    .await
    .unwrap();
    Spawn::call(&*c2, (None, 80u16, 24u16)).await.unwrap();

    // Read the conn ids from `list` (as an operator would).
    let resp = ListChannels::call(&*c1, ()).await.unwrap();
    let ch = resp
        .channels
        .iter()
        .find(|c| c.name == channel)
        .expect("channel listed");
    assert_eq!(ch.clients.len(), 2, "two clients attached");
    let target = ch.clients[0].conn_id;

    // Kill one client through the real CLI subcommand.
    let out = Command::new(bin())
        .env("TERM_WM_GATEWAY", &gateway)
        .args(["kill-client", channel, &target.to_string()])
        .output()
        .expect("run kill-client");
    assert!(
        out.status.success(),
        "kill-client failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // One client remains.
    let resp = ListChannels::call(&*c1, ()).await.unwrap();
    let ch = resp
        .channels
        .iter()
        .find(|c| c.name == channel)
        .expect("channel listed");
    assert_eq!(
        ch.clients.len(),
        1,
        "one client should remain after kill-client"
    );

    ShutdownGateway::call(&*c1, true).await.unwrap();
    let _ = child.wait();
}

#[test]
fn bare_term_session_shows_help_and_does_not_connect() {
    let gateway = unique_gateway("bare");
    let out = Command::new(bin())
        .env("TERM_WM_GATEWAY", &gateway)
        .output()
        .expect("run bare term-session");
    assert_eq!(
        out.status.code(),
        Some(2),
        "bare run must exit 2 (help, not auto-connect), got: {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--channel"),
        "help should mention --channel, got: {stderr}"
    );
    assert!(
        stderr.contains("ls"),
        "help should list the ls subcommand, got: {stderr}"
    );
    assert!(
        stderr.contains("stop"),
        "help should list stop, got: {stderr}"
    );
    // A bare run must NOT have auto-spawned a daemon on the gateway.
    let gw = ChannelName::parse(&gateway).expect("gateway name");
    assert!(
        !probe_ipc_endpoint(&gw),
        "bare run must not auto-spawn a daemon"
    );
}

#[tokio::test]
async fn top_level_channel_auto_attaches() {
    let gateway = unique_gateway("autoattach");
    let channel = "test/autoattach";
    let mock = mock_bin().to_string_lossy().to_string();
    // `term-session --channel <ch> -- <mock> sleep 60000` (no subcommand):
    // giving a channel must still auto-attach and auto-spawn the daemon.
    let mut client = Command::new(bin())
        .env("TERM_WM_GATEWAY", &gateway)
        .args(["--channel", channel, "--", &mock, "sleep", "60000"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn auto-attach client");

    // The auto-spawned daemon becomes reachable and hosts a live session.
    let rpc = wait_connectable(&gateway).await;
    let start = Instant::now();
    loop {
        let resp = ListChannels::call(&*rpc, ()).await.unwrap();
        let live = resp
            .channels
            .iter()
            .any(|c| c.name == channel && c.session.as_ref().is_some_and(|s| !s.exited));
        if live {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "session never appeared on the auto-attached channel"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Cleanup: kill the client process; the daemon (and its live session)
    // survives, so stop it explicitly with force.
    let _ = client.kill();
    let _ = client.wait();
    ShutdownGateway::call(&*rpc, true).await.unwrap();
}

#[tokio::test]
async fn dash_dash_disambiguates_command_from_subcommand() {
    let gateway = unique_gateway("disambig");
    let gw = ChannelName::parse(&gateway).expect("gateway name");

    // `term-session list` (no `--`) parses `list` as the admin SUBCOMMAND: it
    // connects to a gateway and never auto-spawns one.
    let out = Command::new(bin())
        .env("TERM_WM_GATEWAY", &gateway)
        .arg("list")
        .output()
        .expect("run list");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("No gateway"),
        "`list` must parse as the admin subcommand, got: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !probe_ipc_endpoint(&gw),
        "admin subcommand must not auto-spawn"
    );

    // `term-session -- list` (after `--`) parses `list` as a COMMAND to run:
    // the implicit attach path auto-spawns a gateway.
    let mut client = Command::new(bin())
        .env("TERM_WM_GATEWAY", &gateway)
        .args(["--", "list"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run -- list");

    // The auto-spawned gateway becomes reachable (the `list` command itself
    // does not exist, so the session spawn fails — but the daemon persists).
    let rpc = wait_connectable(&gateway).await;
    ShutdownGateway::call(&*rpc, true).await.unwrap();
    let _ = client.kill();
    let _ = client.wait();
}

#[tokio::test]
async fn unknown_flag_errors_without_spawning_gateway() {
    // A leading-hyphen token that is not a real flag (e.g. `--list`, a typo for
    // the `list` subcommand) must be rejected by clap, never swallowed into the
    // trailing command and auto-attached. Each must exit non-zero and leave no
    // gateway behind.
    for flag in ["--list", "--bogus", "-x"] {
        let gateway = unique_gateway("unknown_flag");
        let gw = ChannelName::parse(&gateway).expect("gateway name");
        let out = Command::new(bin())
            .env("TERM_WM_GATEWAY", &gateway)
            .arg(flag)
            .output()
            .expect("run with unknown flag");
        assert_ne!(
            out.status.code(),
            Some(0),
            "`{flag}` must not exit successfully"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("unexpected argument"),
            "`{flag}` must be reported as an unexpected argument, got: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !probe_ipc_endpoint(&gw),
            "`{flag}` must not auto-spawn a gateway"
        );
    }
}

#[tokio::test]
async fn cli_list_renders_client_identity() {
    let gateway = unique_gateway("list_identity");
    let channel = "test/list_identity";
    let (mut daemon, _marker) = spawn_daemon(&gateway, false);

    // A client that stays connected, with explicit identity fields.
    let client = wait_connectable(&gateway).await;
    Attach::call(
        &*client,
        AttachRequest {
            channel: channel.to_string(),
            hostname: "render-host".to_string(),
            pid: 4242,
            user: "bob".to_string(),
            version: "v7".to_string(),
            ssh_ip: Some("203.0.113.9".to_string()),
        },
    )
    .await
    .unwrap();
    Spawn::call(&*client, (None, 80u16, 24u16)).await.unwrap();

    let out = Command::new(bin())
        .env("TERM_WM_GATEWAY", &gateway)
        .arg("list")
        .output()
        .expect("run list");
    assert!(
        out.status.success(),
        "list failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("user: bob"),
        "list must render the client user, got: {stdout}"
    );
    assert!(
        stdout.contains("version: v7"),
        "list must render the client version, got: {stdout}"
    );
    assert!(
        stdout.contains("ssh ip from: 203.0.113.9"),
        "list must render the remote ssh ip, got: {stdout}"
    );

    ShutdownGateway::call(&*client, true).await.unwrap();
    let _ = daemon.wait();
}

#[tokio::test]
async fn cli_stop_requires_force_when_live_sessions() {
    let gateway = unique_gateway("stop_force");
    let channel = "test/stop_force";
    let (mut child, _marker) = spawn_daemon(&gateway, false);

    let client = wait_connectable(&gateway).await;
    attach_to(&client, channel, "cli").await;
    Spawn::call(
        &*client,
        (
            Some(vec![
                mock_bin().to_string_lossy().to_string(),
                "sleep".into(),
                "60000".into(),
            ]),
            80u16,
            24u16,
        ),
    )
    .await
    .unwrap();

    // `stop` without --force: refused, non-zero exit, daemon keeps running.
    let out = Command::new(bin())
        .env("TERM_WM_GATEWAY", &gateway)
        .arg("stop")
        .output()
        .expect("run stop");
    assert!(
        !out.status.success(),
        "stop must refuse while a live session runs"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("live session"),
        "refusal message should mention live sessions, got: {stderr}"
    );

    // Gateway still reachable after the refusal.
    ListChannels::call(&*client, ())
        .await
        .expect("gateway alive");

    // `stop --force` succeeds and the daemon process exits on its own.
    let out = Command::new(bin())
        .env("TERM_WM_GATEWAY", &gateway)
        .args(["stop", "--force"])
        .output()
        .expect("run stop --force");
    assert!(
        out.status.success(),
        "stop --force failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = child.wait();
}
