#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use muxio_tokio_rpc_ipc_client::RpcIpcClient;
use term_session_server::SessionServerConfig;
use term_session_server::run_server;

pub const TEST_COLS: u16 = 80;
pub const TEST_ROWS: u16 = 24;
pub const LONG_SLEEP_MS: u64 = 60000;

#[cfg(unix)]
pub fn generate_socket_path() -> (Option<tempfile::TempDir>, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir
        .path()
        .join("term-wm-test.sock")
        .to_string_lossy()
        .to_string();
    (Some(dir), path)
}

#[cfg(windows)]
pub fn generate_socket_path() -> (Option<tempfile::TempDir>, String) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    (None, format!(r"\\.\pipe\term-wm-test-session-{}", id))
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

pub async fn spawn_session(
    cmd: Vec<String>,
    cols: u16,
    rows: u16,
) -> (Arc<RpcIpcClient>, tempfile::TempDir) {
    let (tempdir, socket_path) = generate_socket_path();
    let config = SessionServerConfig {
        socket_path: socket_path.clone(),
        cmd,
        cols,
        rows,
    };
    tokio::spawn(async move { run_server(config).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let client = connect_client_with_retry(&socket_path).await;
    let dir = tempdir.unwrap_or_else(|| tempfile::tempdir().unwrap());
    (client, dir)
}
