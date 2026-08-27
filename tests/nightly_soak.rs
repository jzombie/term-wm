#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use muxio_tokio_rpc_ipc_client::RpcCallPrebuffered;
use term_session_muxio_service_definitions::{ChannelName, Spawn, SpawnRequest};

mod common;
use common::session::{
    TEST_COLS, TEST_ROWS, attach_client, connect_client_with_retry, list_channels, spawn_gateway,
};

/// Nightly soak: 7 channels, 6 active workloads, polling every 1-2s.
/// Gated by RUN_NIGHTLY_SOAK=1 and #[ignore] so PR CI is unaffected.
#[tokio::test]
#[ignore = "requires RUN_NIGHTLY_SOAK=1"]
async fn nightly_seven_channel_soak() {
    if std::env::var("RUN_NIGHTLY_SOAK").as_deref() != Ok("1") {
        eprintln!("Skipping soak test: RUN_NIGHTLY_SOAK=1 not set");
        return;
    }

    let soak_secs: u64 = std::env::var("SOAK_DURATION_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let soak_duration = Duration::from_secs(soak_secs);

    eprintln!(
        "Running nightly soak for {}s (7 channels, 6 workloads, polling every 1-2s)",
        soak_secs
    );

    let guard = spawn_gateway().await;
    let socket = guard.socket().to_string();

    // Create 7 channels, each with a client
    let mut clients = Vec::new();
    for i in 0..7 {
        let client = connect_client_with_retry(&socket).await;
        let channel = ChannelName::parse(&format!("test/soak-{i}")).expect("parse");
        attach_client(&client, &channel).await;
        // 6 of them get a long-running workload, 1 is idle but still needs a Spawn
        // to populate ChannelState.clients (otherwise it would be reaped as empty)
        let cmd = if i < 6 {
            Some(vec!["sleep".to_string(), "60000".to_string()])
        } else {
            None
        };
        let _ = Spawn::call(
            &*client,
            SpawnRequest {
                cmd,
                cols: TEST_COLS,
                rows: TEST_ROWS,
                cwd: None,
            },
        )
        .await;
        clients.push((client, channel));
    }

    let start = Instant::now();
    let mut polls = 0;
    let mut last_rss_check = Instant::now();

    while start.elapsed() < soak_duration {
        // Poll admin RPCs every 1-2s (mimics term-wm app + internal session)
        let poll_client = &clients[0].0;
        let channels = list_channels(poll_client).await;
        assert_eq!(
            channels.channels.len(),
            7,
            "all 7 channels should still be listed"
        );

        // Check that CLIENT_PENDING_WRITES would be 0 after a burst is drained
        // (we don't have direct access here, but we can at least verify the daemon answers)

        polls += 1;
        if last_rss_check.elapsed() > Duration::from_secs(5) {
            eprintln!("soak poll #{polls} at {:?}", start.elapsed());
            last_rss_check = Instant::now();
        }

        // Interleaved bursts: simulate ResizePty and ReportWmStats would go here
        tokio::time::sleep(Duration::from_millis(800)).await;
    }

    eprintln!(
        "Soak completed after {:?} with {} polls",
        start.elapsed(),
        polls
    );
    // Assert daemon still answers after soak
    let final_channels = list_channels(&clients[0].0).await;
    assert_eq!(final_channels.channels.len(), 7);

    guard.shutdown().await;
}
