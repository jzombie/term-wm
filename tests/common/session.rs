#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use muxio_tokio_rpc_ipc_client::{RpcCallPrebuffered, RpcIpcClient};
use term_session_muxio_service_definitions::{
    Attach, AttachRequest, ChannelName, ListChannels, ShutdownGateway,
};
use term_session_server::run_gateway;
use term_test_support::unique_gateway_name;

pub const TEST_COLS: u16 = 80;
pub const TEST_ROWS: u16 = 24;
pub const LONG_SLEEP_MS: u64 = 60000;
/// Generous deadline for "child produced expected output" round-trips.
/// Polling exits the moment the condition holds, so a large deadline is free
/// on success and only failure paths pay it; a tight deadline instead turns
/// CI-runner load spikes into flakes (issue #309).
pub const LIVENESS_TIMEOUT: Duration = Duration::from_secs(15);

pub fn test_channel(name: &str) -> ChannelName {
    ChannelName::parse(name).expect("test channel")
}

// ---------------------------------------------------------------------------
// GatewayGuard — RAII cleanup for in-process gateways
//
// Root cause: `#[tokio::test]` uses a `current_thread` runtime. When the
// test body returns, `Runtime::drop` does NOT cancel spawned tasks. The
// `run_gateway` future (spawned via `tokio::spawn`) survives the runtime
// teardown, holding `Arc<ServerState>` → `ChannelState` → `Session` → `Pty`
// → child process alive. The leaked mock children (echo, sleep) keep
// running until they are manually killed or the OS reclaims them.
//
// The only reliable way to terminate a gateway is to call `ShutdownGateway`
// over the IPC channel. This RPC kills every session's child process, then
// signals the `run_gateway` future to return, which drops `ServerState` and
// all associated state.
//
// `GatewayGuard` wraps the client connection and provides an explicit
// `shutdown()` method. Every test that spawns a gateway or session MUST
// store the guard and call `guard.shutdown().await` before the test ends.
// ---------------------------------------------------------------------------

#[must_use = "gateway must be shut down via .shutdown().await to avoid leaking mock processes"]
pub struct GatewayGuard {
    client: Arc<RpcIpcClient>,
    socket: String,
    shut_down: bool,
}

impl GatewayGuard {
    /// Returns the underlying RPC client for this gateway.
    pub fn client(&self) -> &Arc<RpcIpcClient> {
        &self.client
    }

    /// Returns the IPC socket path (useful for connecting additional clients).
    pub fn socket(&self) -> &str {
        &self.socket
    }

    /// Shut down the gateway, killing all child processes and dropping
    /// `ServerState`. Must be called before the test ends.
    pub async fn shutdown(mut self) {
        shutdown_gateway(&self.client).await;
        self.shut_down = true;
    }
}

impl Drop for GatewayGuard {
    fn drop(&mut self) {
        // Panic-safety diagnostic: `shutdown().await` is the only reliable
        // teardown (see the block comment above), so reaching this Drop means
        // the test ended without calling it, typically via an assertion
        // failure. The leaked in-process gateway task dies with the test
        // binary, but its mock child processes can linger until then. Print a
        // loud marker so CI logs pinpoint the offending test instead of the
        // leak staying invisible.
        if !self.shut_down {
            eprintln!(
                "WARNING: GatewayGuard dropped without shutdown(); gateway '{}' \
                 may hold mock child processes until the test process exits",
                self.socket
            );
        }
    }
}

/// Spawn one shared gateway daemon for the current test, using a unique
/// gateway name so parallel tests never collide. The name embeds the process
/// id (via [`unique_gateway_name`]), so concurrent test binaries and leftover
/// daemons from crashed prior runs cannot claim the same endpoint. Returns a
/// [`GatewayGuard`] that MUST be shut down via `.shutdown().await` before the
/// test ends.
pub async fn spawn_gateway() -> GatewayGuard {
    let gateway = ChannelName::parse(&unique_gateway_name("testgw")).expect("unique gateway");
    tokio::spawn({
        let gateway = gateway.clone();
        async move { run_gateway(gateway).await }
    });
    // No fixed startup delay here: `connect_client_with_retry` polls the
    // socket on a deadline, which is the actual readiness signal we care
    // about; sleeping first would only pad every test's runtime.
    let socket = gateway.to_string();
    let client = connect_client_with_retry(&socket).await;
    GatewayGuard {
        client,
        socket,
        shut_down: false,
    }
}

pub fn get_bench_bin() -> PathBuf {
    let mut path = std::env::current_exe().expect("test exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(format!("term-bench{}", std::env::consts::EXE_SUFFIX));
    path
}

pub async fn connect_client_with_retry(socket_path: &str) -> Arc<RpcIpcClient> {
    let timeout = Duration::from_secs(3);
    let start = std::time::Instant::now();
    loop {
        match RpcIpcClient::new(socket_path).await {
            Ok(client) => return client,
            Err(e) if start.elapsed() < timeout => {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let _ = e;
            }
            Err(e) => panic!("Failed to connect to server after {timeout:?}: {e}"),
        }
    }
}

/// Attach a client to a channel, returning its server-assigned conn id.
pub async fn attach_client(client: &RpcIpcClient, channel: &ChannelName) -> usize {
    Attach::call(
        client,
        AttachRequest {
            channel: channel.to_string(),
            hostname: "test-host".to_string(),
            pid: std::process::id() as u64,
            user: "test-user".to_string(),
            version: "test-version".to_string(),
            ssh_ip: None,
            ssh_port: None,
        },
    )
    .await
    .expect("attach")
}

pub async fn wait_for_output(
    reader: &mut tokio::sync::mpsc::UnboundedReceiver<
        Result<Vec<u8>, muxio_rpc_service::error::RpcServiceError>,
    >,
    pattern: &[u8],
    timeout: Duration,
) -> Vec<u8> {
    let start = std::time::Instant::now();
    let mut accumulated = Vec::new();
    loop {
        let remaining = timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, reader.recv()).await {
            Ok(Some(Ok(data))) => {
                accumulated.extend_from_slice(&data);
                if accumulated.windows(pattern.len()).any(|w| w == pattern) {
                    break;
                }
            }
            Ok(Some(Err(_))) => break,
            Ok(None) => break,
            Err(_) => break,
        }
    }
    accumulated
}

/// Convenience: spawn the gateway, connect, and attach to a channel,
/// returning `(client, conn_id, guard)`. The guard MUST be shut down
/// via `.shutdown().await` before the test ends.
pub async fn spawn_session(channel: &ChannelName) -> (Arc<RpcIpcClient>, usize, GatewayGuard) {
    let guard = spawn_gateway().await;
    let conn_id = attach_client(guard.client(), channel).await;
    (guard.client().clone(), conn_id, guard)
}

/// List channels via the admin method (no attach required).
pub async fn list_channels(
    client: &Arc<RpcIpcClient>,
) -> term_session_muxio_service_definitions::ListChannelsResponse {
    ListChannels::call(&**client, ())
        .await
        .expect("list channels")
}

/// Stop the gateway daemon.
pub async fn shutdown_gateway(client: &Arc<RpcIpcClient>) {
    ShutdownGateway::call(&**client, true)
        .await
        .expect("shutdown");
}
