#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use muxio_tokio_rpc_ipc_client::{RpcCallPrebuffered, RpcIpcClient};
use term_session_muxio_service_definitions::{Attach, ChannelName, ListChannels, ShutdownGateway};
use term_session_server::run_gateway;

pub const TEST_COLS: u16 = 80;
pub const TEST_ROWS: u16 = 24;
pub const LONG_SLEEP_MS: u64 = 60000;

pub fn test_channel(name: &str) -> ChannelName {
    ChannelName::parse(name).expect("test channel")
}

/// Spawn one shared gateway daemon for the current test, using a unique
/// gateway name so parallel tests never collide. Returns the gateway socket
/// name string that clients connect to.
pub async fn spawn_gateway() -> String {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let gateway = ChannelName::parse(&format!("term-wm/testgw-{id}")).expect("unique gateway");
    tokio::spawn({
        let gateway = gateway.clone();
        async move { run_gateway(gateway).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    connect_client_with_retry(&gateway.to_string()).await;
    gateway.to_string()
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
        (
            channel.to_string(),
            "test-host".to_string(),
            std::process::id() as u64,
        ),
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
/// returning `(client, conn_id)`.
pub async fn spawn_session(channel: &ChannelName) -> (Arc<RpcIpcClient>, usize) {
    let gateway = spawn_gateway().await;
    let client = connect_client_with_retry(&gateway).await;
    let conn_id = attach_client(&client, channel).await;
    (client, conn_id)
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
    ShutdownGateway::call(&**client, ())
        .await
        .expect("shutdown");
}
