#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use muxio_tokio_rpc_ipc_client::RpcIpcClient;
use term_session_muxio_service_definitions::ChannelName;
use term_session_server::{SessionServerConfig, run_server};

pub const TEST_COLS: u16 = 80;
pub const TEST_ROWS: u16 = 24;
pub const LONG_SLEEP_MS: u64 = 60000;

pub fn test_channel(name: &str) -> ChannelName {
    ChannelName::parse(name).expect("test channel")
}

pub async fn spawn_server_for_channel(
    channel: &ChannelName,
    cmd: Vec<String>,
) -> Arc<RpcIpcClient> {
    let config = SessionServerConfig {
        channel: channel.clone(),
        cmd,
    };
    tokio::spawn(async move { run_server(config).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    connect_client_with_retry(&channel.to_string()).await
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

pub async fn spawn_session(channel: &ChannelName, cmd: Vec<String>) -> Arc<RpcIpcClient> {
    spawn_server_for_channel(channel, cmd).await
}
