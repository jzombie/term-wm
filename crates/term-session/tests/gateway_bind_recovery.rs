//! Regression tests: the daemon bind gate must never take over an
//! endpoint owned by a live process, and must recover stale socket files.
//!
//! Pins the fix for cross-generation gateway endpoint takeover: before the
//! fix, a probe timeout caused client-side auto-spawn to delete the live
//! socket file and bind a fresh wrong-generation daemon over the name
//! (split-brain: old daemon alive but unaddressable).

use std::path::{Path, PathBuf};
use std::time::Duration;

use muxio_tokio_rpc_ipc_client::{RpcCallPrebuffered, RpcIpcClient};
use term_session_muxio_service_definitions::{
    ChannelName, ProbeOutcome, ShutdownGateway, probe_endpoint_outcome,
};
use term_session_server::run_gateway;
use term_test_support::unique_gateway_name;

/// RAII cleanup for socket-path artifacts so failed assertions never leave
/// debris behind for subsequent local runs. `None` on platforms without a
/// filesystem artifact (Linux abstract namespace).
struct PathGuard(Option<PathBuf>);

impl PathGuard {
    fn new(path: Option<PathBuf>) -> Self {
        Self(path)
    }

    fn path(&self) -> Option<&Path> {
        self.0.as_deref()
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Mirrors the interprocess `GenericNamespaced` -> filesystem mapping for
/// socket-path assertions: `/tmp/<name>` on macOS/BSD (the crate's
/// "special directory"), nothing on Linux (abstract namespace keeps no
/// filesystem artifact), `\\.\pipe\<name>` on Windows named pipes.
#[cfg(target_os = "linux")]
fn socket_path(_gateway: &str) -> Option<PathBuf> {
    None
}

#[cfg(not(target_os = "linux"))]
fn socket_path(gateway: &str) -> Option<PathBuf> {
    if cfg!(windows) {
        return Some(PathBuf::from(format!(r"\\.\pipe\{gateway}")));
    }
    Some(PathBuf::from("/tmp").join(gateway))
}

/// Bind a real listener on the gateway name and NEVER service its accept
/// queue: the stand-in for a live-but-CPU-starved daemon owner.
///
/// The handle must stay in scope for the whole assertion window; dropping
/// it closes the kernel socket and would fabricate a false refusal.
async fn spawn_never_accepting_owner(
    gateway: &str,
) -> term_test_support::KillOnDrop<Box<dyn FnOnce() + Send>> {
    use interprocess::local_socket::traits::tokio::Listener as _;
    use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};
    let name = gateway.to_ns_name::<GenericNamespaced>().expect("ns name");
    let listener = ListenerOptions::new()
        .name(name)
        .create_tokio()
        .expect("bind owner");

    let handle = tokio::spawn(async move {
        while let Ok(_stream) = listener.accept().await {
            // Accept and immediately drop each stream: endpoint stays bound
            // but never serves anything (the "busy owner" state under test).
        }
    });

    term_test_support::KillOnDrop::new(Box::new(move || handle.abort()))
}

/// Poll `probe_endpoint_outcome` until it reports `Live`, 50 ms cadence
/// bounded to a 5-second budget.
async fn wait_until_live(gateway: &ChannelName) -> bool {
    for _ in 0..100 {
        if matches!(probe_endpoint_outcome(gateway), Ok(ProbeOutcome::Live)) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn gateway_startup_refuses_to_take_over_live_endpoint() {
    let gw_str = unique_gateway_name("regr-live");
    let gw = ChannelName::parse(&gw_str).expect("parse");
    // Declaration order is load-bearing: path_guard is dropped LAST (after
    // the kernel socket closed), _owner FIRST. RAII-only: exclusivity is
    // asserted behaviorally below, never via filesystem inspection.
    let _path_guard = PathGuard::new(socket_path(&gw_str));
    let _owner = spawn_never_accepting_owner(&gw_str).await;

    assert!(
        matches!(probe_endpoint_outcome(&gw), Ok(ProbeOutcome::Live)),
        "precondition: owner must answer"
    );

    // Task-abort semantics: timeout cancels the WAIT, never the task;
    // always abort the join handle afterwards so no server loop leaks into
    // subsequent tests.
    let mut task = tokio::spawn(run_gateway(gw.clone()));
    let outcome = tokio::time::timeout(Duration::from_secs(5), &mut task).await;
    task.abort(); // no-op when already finished

    let result = outcome
        .expect("gate must fail fast, not hang")
        .expect("gate task must not panic");
    let err = result.expect_err("startup must refuse takeover");
    assert!(
        err.to_string().contains("busy"),
        "expected busy-owner refusal, got: {err}"
    );

    // Owner untouched: exclusivity is behavioral — it still answers, and
    // startup refused rather than taking over. Filesystem-artifact checks
    // would leak transport internals here: UDS leaves socket nodes in the
    // VFS on POSIX, while Windows named pipes live purely in kernel memory,
    // so disk-presence assertions audit the OS socket representation rather
    // than binary behavior.
    assert!(
        matches!(probe_endpoint_outcome(&gw), Ok(ProbeOutcome::Live)),
        "original owner must remain the sole endpoint"
    );
}

/// Unix-only by construction: orphaned UDS socket files left on disk after
/// an ungraceful crash are a POSIX quirk — Windows reclaims named-pipe
/// instances the moment process handles close, so there is no stale
/// filesystem artifact for the recovery gate to clean up there.
#[cfg(unix)]
#[tokio::test]
async fn gateway_startup_recovers_stale_socket_file() {
    let gw_str = unique_gateway_name("regr-stale");
    let gw = ChannelName::parse(&gw_str).expect("parse");
    let path_guard = PathGuard::new(socket_path(&gw_str));

    // Stale leftover: a socket FILE with nothing listening behind it, exactly what a hard-crashed daemon leaves behind.
    // exactly what a hard-crashed daemon leaves behind. Platforms without
    // a filesystem artifact (Linux abstract namespace) start in the
    // NotFound flavor of Stale automatically.
    if let Some(path) = path_guard.path() {
        std::fs::write(path, b"stale artifact").expect("write stale");
    }

    let mut task = tokio::spawn(run_gateway(gw.clone()));

    // Explicit assertion BEFORE any RPC: recovery gate must unlink the
    // stale file and come up serving within budget.
    if !wait_until_live(&gw).await {
        let sock_exists = path_guard.path().is_some_and(|p| p.exists());
        let diag = match tokio::time::timeout(Duration::from_secs(2), &mut task).await {
            Ok(joined) => format!("task ended: {joined:?}"),
            Err(_) => "task STILL RUNNING".to_string(),
        };
        panic!("recovery failed: stale-file-exists={sock_exists} {diag}");
    }

    // Clean shutdown through the real RPC surface.
    let client = RpcIpcClient::new(&gw_str).await.expect("client connect");
    ShutdownGateway::call(&*client, true)
        .await
        .expect("shutdown");
    let code = task
        .await
        .expect("gateway task completes")
        .expect("clean exit");
    assert_eq!(code, 0);
}
