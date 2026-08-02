//! Binary/daemon tests for the `term-session` gateway.
//!
//! These exercise the real compiled binary (`CARGO_BIN_EXE_term-session`):
//! detachment proof via `--daemon-selfcheck`, daemon resilience to client
//! disconnects and parent death, and clean teardown via `ShutdownGateway`.
//!
//! Each test uses a unique `TERM_WM_GATEWAY` so parallel runs never collide.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use muxio_tokio_rpc_ipc_client::RpcCallPrebuffered;
use term_session_muxio_service_definitions::{Attach, ShutdownGateway, Spawn};

/// The compiled `term-session` binary under test.
fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_term-session"))
}

/// Path to the mock PTY binary used as a session child.
fn mock_bin() -> PathBuf {
    let mut path = std::env::current_exe().expect("test exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(format!("term-session-mock{}", std::env::consts::EXE_SUFFIX));
    path
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

/// Poll until a client can connect to the gateway, or panic after a timeout.
async fn wait_connectable(gateway: &str) -> Arc<muxio_tokio_rpc_ipc_client::RpcIpcClient> {
    let start = Instant::now();
    loop {
        match muxio_tokio_rpc_ipc_client::RpcIpcClient::new(gateway).await {
            Ok(c) => return c,
            Err(_) if start.elapsed() < Duration::from_secs(8) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("gateway {gateway} not reachable: {e}"),
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
    ShutdownGateway::call(&*client, ()).await.unwrap();
    let _ = child.wait();
}

#[tokio::test]
async fn daemon_survives_all_clients_disconnecting() {
    let gateway = unique_gateway("survive");
    let (mut child, _marker) = spawn_daemon(&gateway, false);

    let client = wait_connectable(&gateway).await;
    let channel = "test/daemon_survive";
    Attach::call(
        &*client,
        (
            channel.to_string(),
            "t".to_string(),
            std::process::id() as u64,
        ),
    )
    .await
    .unwrap();
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
    Attach::call(
        &*client2,
        (
            channel.to_string(),
            "t".to_string(),
            std::process::id() as u64,
        ),
    )
    .await
    .unwrap();
    Spawn::call(&*client2, (None, 80u16, 24u16)).await.unwrap();

    ShutdownGateway::call(&*client2, ()).await.unwrap();
    let _ = child.wait();
}

#[tokio::test]
async fn daemon_survives_parent_death() {
    let gateway = unique_gateway("parent_death");
    let channel = "test/daemon_parent_death";
    let mock = mock_bin().to_string_lossy().to_string();

    // Spawn an `attach` subprocess that auto-spawns the daemon, then kill it.
    let mut attach = Command::new(bin())
        .env("TERM_WM_GATEWAY", &gateway)
        .args(["attach", "--channel", channel, "--", &mock, "echo"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn attach");

    // Give it time to auto-spawn the daemon and attach.
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let _ = attach.kill();
    let _ = attach.wait();

    // The daemon it spawned must still be reachable and the session alive.
    let client = wait_connectable(&gateway).await;
    Attach::call(
        &*client,
        (
            channel.to_string(),
            "t".to_string(),
            std::process::id() as u64,
        ),
    )
    .await
    .unwrap();
    let (id, _, _) = Spawn::call(&*client, (None, 80u16, 24u16)).await.unwrap();
    assert_eq!(id, 1, "session from the orphaned daemon must persist");

    ShutdownGateway::call(&*client, ()).await.unwrap();
    // Give the daemon time to run its teardown and exit.
    tokio::time::sleep(Duration::from_millis(1000)).await;
}
