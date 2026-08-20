#![allow(clippy::unwrap_used)]

use muxio_rpc_service::prebuffered::RpcMethodPrebuffered;
use muxio_tokio_mpsc_adapter::ChannelCallerExt;
use muxio_tokio_rpc_ipc_client::{RpcCallPrebuffered, RpcIpcClient};
use serial_test::serial;
use std::sync::Arc;
use std::time::Duration;
use term_session_muxio_service_definitions::{
    Attach, AttachRequest, CloseSession, KillChannel, KillClient, ResizePty,
    STREAM_INPUT_METHOD_ID, SUBSCRIBE_OUTPUT_METHOD_ID, ShutdownGateway, Spawn, SpawnRequest,
    SpawnResponse,
};

mod common;
use common::mock::{
    EXPECTED_OSC52_PAYLOAD, find_osc52_payload, find_sgr_mouse_token, get_mock_bin, mock_pid_alive,
};
use common::session::{
    TEST_COLS, TEST_ROWS, attach_client, connect_client_with_retry, get_bench_bin, list_channels,
    spawn_gateway, spawn_session, test_channel, wait_for_output,
};
use term_clipboard::Osc52Extractor;

#[tokio::test]
async fn session_spawn_returns_id() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/spawn_returns_id")).await;
    let SpawnResponse {
        id,
        cols: _,
        rows: _,
    } = Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "echo".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(id, 1);
    guard.shutdown().await;
}

#[tokio::test]
async fn session_input_output_roundtrip() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/input_output")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "echo".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let (_, mut reader) = client
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();
    let (writer, _) = client
        .open_channel(STREAM_INPUT_METHOD_ID, 0)
        .await
        .unwrap();
    writer.send(b"hello\n".to_vec()).unwrap();

    let output = wait_for_output(&mut reader, b"hello", Duration::from_secs(3)).await;
    assert!(
        output.windows(5).any(|w| w == b"hello"),
        "Expected 'hello' in output, got: {:?}",
        String::from_utf8_lossy(&output)
    );
    guard.shutdown().await;
}

/// Regression test for the input-reordering bug observed with IME voice typing
/// over SSH (termux + Google voice typing): a burst of many stream input
/// chunks must reach the PTY in the exact order they were sent.
///
/// The original server `StreamInput` handler spawned an independent tokio task
/// per chunk, and those tasks raced on the async routing locks — so under a
/// burst, later chunks could be written to the PTY before earlier ones. The
/// fix forwards chunks synchronously into a per-connection ordered queue
/// drained FIFO by a single task.
///
/// `echo` echoes stdin back verbatim, so the PTY output must reproduce the
/// sent byte sequence exactly. Each marker is a distinct printable string so
/// any reordering is observable in the echoed stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_stream_input_preserves_order_under_burst() {
    const BURST_COUNT: usize = 64;
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/input_burst_order")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "echo".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let (_, mut reader) = client
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();
    let (writer, _) = client
        .open_channel(STREAM_INPUT_METHOD_ID, 0)
        .await
        .unwrap();

    // Build a deterministic ordered sequence of distinct markers.
    let mut markers = Vec::new();
    for i in 0..BURST_COUNT {
        let marker = format!("m{i:03}\n").into_bytes();
        markers.push(marker);
    }

    // Fire the whole burst back-to-back WITHOUT awaiting between sends, so
    // many chunks are in flight concurrently at the gateway (reproduces the
    // voice-typing burst).
    for marker in &markers {
        writer.send(marker.clone()).unwrap();
    }

    // Read the echoed output. The PTY echoes input AND `echo` mirrors it, and
    // line discipline mangles newlines (`\n` -> `\r\n`), so each marker's bytes
    // appear (at least) twice and in mangled form. The robust signal is the
    // ORDER in which markers first appear: it must equal the sent order.
    let output = wait_for_output(
        &mut reader,
        &markers.last().unwrap()[..4],
        Duration::from_secs(5),
    )
    .await;
    assert_markers_in_first_appearance_order(&output, &markers);

    guard.shutdown().await;
}

/// Assert that the distinct markers appear in `output` in the same order they
/// were sent, using first-appearance order. Echo duplication/mangling makes a
/// plain contiguous or subsequence match unreliable, but the first occurrence
/// of each distinct marker faithfully reflects write order.
fn assert_markers_in_first_appearance_order(output: &[u8], markers: &[Vec<u8>]) {
    let needles: Vec<&[u8]> = markers.iter().map(|m| &m[..m.len() - 1]).collect();
    let mut found = Vec::new();
    let mut i = 0usize;
    while i < output.len() {
        for (idx, needle) in needles.iter().enumerate() {
            if i + needle.len() <= output.len() && &output[i..i + needle.len()] == *needle {
                if !found.contains(&idx) {
                    found.push(idx);
                }
                break;
            }
        }
        i += 1;
    }
    let expected_order: Vec<usize> = (0..needles.len()).collect();
    assert_eq!(
        found,
        expected_order,
        "input burst was reordered: expected markers in order {:?}, got first-appearance order {:?}; output: {:?}",
        expected_order,
        found,
        String::from_utf8_lossy(output)
    );
}

#[cfg(not(windows))]
#[tokio::test]
async fn session_mouse_bytes_forwarded() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/mouse_forward")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "echo".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let (_, mut reader) = client
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();
    let (writer, _) = client
        .open_channel(STREAM_INPUT_METHOD_ID, 0)
        .await
        .unwrap();

    let mouse_bytes = b"\x1b[<0;5;10M";
    writer.send(mouse_bytes.to_vec()).unwrap();
    writer.send(b"\n".to_vec()).unwrap();

    let output = wait_for_output(&mut reader, b"\x1b[<", Duration::from_secs(3)).await;
    let token = find_sgr_mouse_token(&output);
    assert!(
        token.is_some(),
        "PTY output missing complete SGR 1006 mouse sequence, got {} bytes",
        output.len(),
    );
    let token = token.unwrap();
    let params = &token[3..token.len() - 1];
    assert_eq!(
        params,
        b"0;5;10",
        "Mouse token params mismatch: expected '0;5;10', got {:?}",
        String::from_utf8_lossy(params)
    );
    guard.shutdown().await;
}

/// A rapid drag burst (100 SGR mouse packets) must survive the coalescing
/// forwarder intact: the final event must reach the PTY and be echoed back.
#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_mouse_drag_burst_throughput() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/mouse_burst")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "echo".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let (_, mut reader) = client
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();
    let (writer, _) = client
        .open_channel(STREAM_INPUT_METHOD_ID, 0)
        .await
        .unwrap();

    for i in 0..100 {
        let mouse_event = format!("\x1b[<32;{i};10M\n").into_bytes();
        writer.send(mouse_event).unwrap();
    }

    let output = wait_for_output(&mut reader, b"32;99;10M", Duration::from_secs(3)).await;
    assert!(
        output.windows(9).any(|w| w == b"32;99;10M"),
        "Expected final mouse event in output stream"
    );

    guard.shutdown().await;
}

/// On Windows, ConPTY intercepts escape sequences written to the PTY master's
/// stdin pipe before they reach the child process. The `capture` subcommand
/// verifies the PTY input→output pipeline using a `MOUSE_OK:` sentinel marker
/// instead of raw escape sequences.
#[cfg(windows)]
#[tokio::test]
async fn session_mouse_bytes_forwarded() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/mouse_forward")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "capture".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let (_, mut reader) = client
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();
    let (writer, _) = client
        .open_channel(STREAM_INPUT_METHOD_ID, 0)
        .await
        .unwrap();

    // NOTE: must include trailing \n — on headless Windows CI where
    // GetConsoleMode fails, stdin defaults to line-input mode and
    // blocks until a newline is received.
    writer.send(b"ping\n".to_vec()).unwrap();

    let output = wait_for_output(&mut reader, b"MOUSE_OK:", Duration::from_secs(3)).await;
    assert!(
        output.windows(9).any(|w| w == b"MOUSE_OK:"),
        "PTY input pipeline broken on Windows, got {} bytes: {:?}",
        output.len(),
        String::from_utf8_lossy(&output)
    );
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn session_osc52_in_output() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/osc52_output")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "osc52_alive".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let (_, mut reader) = client
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();

    // Wait for the complete payload (not just the `52;` header) so a payload
    // split across broadcast chunks can never break the wait early.
    let output = wait_for_output(&mut reader, EXPECTED_OSC52_PAYLOAD, Duration::from_secs(3)).await;
    let payload = find_osc52_payload(&output);
    assert_eq!(
        payload,
        Some(common::mock::EXPECTED_OSC52_PAYLOAD),
        "OSC 52 payload extraction failed, got stream: {:?}",
        String::from_utf8_lossy(&output)
    );
    guard.shutdown().await;
}

// Note: [serial] was added due to some Windows flakiness
#[tokio::test]
#[serial]
async fn session_osc52_via_osc52extractor() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/osc52_extractor")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "osc52_alive".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let (_, mut reader) = client
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();

    let mut extractor = Osc52Extractor::new();
    let mut prev_tail: [u8; 8] = [0; 8];
    let mut extracted = None;

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        match tokio::time::timeout(Duration::from_millis(200), reader.recv()).await {
            Ok(Some(Ok(data))) => {
                if let Some(text) = extractor.push(&data, &prev_tail) {
                    extracted = Some(text);
                    break;
                }
                let n = data.len();
                if n >= 8 {
                    prev_tail.copy_from_slice(&data[n - 8..n]);
                } else if n > 0 {
                    prev_tail.rotate_left(n);
                    prev_tail[8 - n..].copy_from_slice(&data[..n]);
                }
            }
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => continue,
        }
    }

    // End-of-stream: Windows ConPTY consumes the BEL terminator, so the
    // payload may be the last bytes with no terminator. Flush it.
    if extracted.is_none() {
        extracted = extractor.finish();
    }

    assert_eq!(
        extracted,
        Some("test".to_string()),
        "Osc52Extractor should decode 'test' from real server byte stream"
    );
    guard.shutdown().await;
}

/// Regression: a subscriber that attaches AFTER a session has exited (with no
/// subscribers attached at exit time) must still receive the session's final
/// output, which the server retains in a bounded cache across teardown instead
/// of dropping it with the Pty's pending buffer.
#[tokio::test]
#[serial]
async fn session_osc52_late_subscribe_gets_retained_output() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/osc52_retained")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "osc52".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    // The `osc52` mock writes the payload, sleeps 500 ms, then exits. Wait well
    // past exit + the polling task's detection interval so the session is torn
    // down and its final output retained in the channel cache. The test is
    // deterministic either way: subscribe-before-exit is served by the live
    // early drain, subscribe-after-exit by the retained cache.
    tokio::time::sleep(Duration::from_millis(1200)).await;

    let (_, mut reader) = client
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();
    let output = wait_for_output(&mut reader, EXPECTED_OSC52_PAYLOAD, Duration::from_secs(3)).await;
    let payload = find_osc52_payload(&output);
    assert_eq!(
        payload,
        Some(common::mock::EXPECTED_OSC52_PAYLOAD),
        "late subscriber must receive retained final output, got stream: {:?}",
        String::from_utf8_lossy(&output)
    );
    guard.shutdown().await;
}

#[tokio::test]
async fn session_resize() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/resize")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "echo".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let result = ResizePty::call(&*client, (1u64, 120u16, 40u16)).await;
    assert!(result.is_ok(), "Resize should succeed: {:?}", result.err());
    guard.shutdown().await;
}

#[tokio::test]
async fn session_list_channels() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/list_channels")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "sleep".into(), "60000".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let resp = list_channels(&client).await;
    let channels = &resp.channels;
    assert!(channels.iter().any(|c| c.name == "test/list_channels"));
    let ch = channels
        .iter()
        .find(|c| c.name == "test/list_channels")
        .unwrap();
    assert!(ch.session.is_some(), "session should be present");
    // Process visibility: the response identifies the daemon PID + socket.
    assert!(resp.gateway_pid > 0, "gateway pid reported");
    assert!(!resp.socket.is_empty(), "socket name reported");
    guard.shutdown().await;
}

#[tokio::test]
async fn session_close_session() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/close_session")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "sleep".into(), "60000".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    CloseSession::call(&*client, 1u64).await.unwrap();

    // CloseSession signals SIGTERM; the output-polling task clears the session
    // once the child reaps, so poll briefly for the channel to report no session.
    let start = std::time::Instant::now();
    loop {
        let channels = list_channels(&client).await;
        let ch = channels
            .channels
            .iter()
            .find(|c| c.name == "test/close_session");
        let gone = ch.map(|c| c.session.is_none()).unwrap_or(true);
        if gone {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "session should be removed after close"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    guard.shutdown().await;
}

// Note: [serial] was added due to some Windows flakiness
#[tokio::test]
#[serial]
async fn session_child_exit() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/child_exit")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "exit".into(), "0".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let (_, mut reader) = client
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();

    let mut got_end = false;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        match tokio::time::timeout(Duration::from_millis(500), reader.recv()).await {
            Ok(None) => {
                got_end = true;
                break;
            }
            Ok(Some(_)) => continue,
            Err(_) => continue,
        }
    }
    assert!(got_end, "Stream should end when child exits");
    guard.shutdown().await;
}

// Note: [serial] was added due to some Windows flakiness
#[tokio::test]
#[serial]
async fn session_child_exit_before_subscribe() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/child_exit_early")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "exit".into(), "0".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    // Give the server time to detect child exit and tear down the session
    tokio::time::sleep(Duration::from_millis(500)).await;

    let (_, mut reader) = client
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();

    let mut got_end = false;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        match tokio::time::timeout(Duration::from_millis(500), reader.recv()).await {
            Ok(None) => {
                got_end = true;
                break;
            }
            Ok(Some(_)) => continue,
            Err(_) => continue,
        }
    }
    assert!(
        got_end,
        "Should get end-of-stream when subscribing after child exit"
    );
    guard.shutdown().await;
}

#[tokio::test]
async fn session_reconnect() {
    let mock = get_mock_bin();
    let guard = spawn_gateway().await;
    let channel = test_channel("test/reconnect");

    let client1 = connect_client_with_retry(guard.socket()).await;
    attach_client(&client1, &channel).await;
    Spawn::call(
        &*client1,
        SpawnRequest {
            cmd: Some(vec![mock.clone(), "echo".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();
    let (_, mut reader1) = client1
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();
    let (writer1, _) = client1
        .open_channel(STREAM_INPUT_METHOD_ID, 0)
        .await
        .unwrap();
    writer1.send(b"one\n".to_vec()).unwrap();
    let output1 = wait_for_output(&mut reader1, b"one", Duration::from_secs(2)).await;
    assert!(output1.windows(3).any(|w| w == b"one"));
    drop(client1);

    let client2 = connect_client_with_retry(guard.socket()).await;
    attach_client(&client2, &channel).await;
    let (_, mut reader2) = client2
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();
    let (writer2, _) = client2
        .open_channel(STREAM_INPUT_METHOD_ID, 0)
        .await
        .unwrap();
    writer2.send(b"two\n".to_vec()).unwrap();
    let output2 = wait_for_output(&mut reader2, b"two", Duration::from_secs(2)).await;
    assert!(output2.windows(3).any(|w| w == b"two"));
    guard.shutdown().await;
}

#[tokio::test]
async fn term_bench_runs_to_completion() {
    let bench_bin = get_bench_bin();
    if !bench_bin.exists() {
        eprintln!("Skipping term_bench test: binary not found at {bench_bin:?}");
        return;
    }

    let (client, _conn_id, guard) = spawn_session(&test_channel("test/bench")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![
                bench_bin.to_string_lossy().to_string(),
                "-d".into(),
                "1".into(),
                "-f".into(),
                "10".into(),
            ]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let (sender, mut reader) = client
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();

    let mut got_end = false;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        match tokio::time::timeout(Duration::from_secs(2), reader.recv()).await {
            Ok(None) => {
                got_end = true;
                break;
            }
            Ok(Some(_)) => continue,
            Err(_) => continue,
        }
    }
    drop(sender);
    assert!(got_end, "term-bench should exit within 10 seconds");
    guard.shutdown().await;
}

#[test]
fn find_sgr_mouse_token_static() {
    let stream = b"\x1b[H\x1b[J\x1b[<0;5;10M\x1b[0m";
    let token = find_sgr_mouse_token(stream).expect("Static SGR mouse token parsing failed");

    assert!(token.len() >= 4);
    let params = &token[3..token.len() - 1];
    assert_eq!(params, b"0;5;10");
}

#[tokio::test]
#[serial]
async fn two_channels_run_concurrently_with_isolated_io() {
    let mock = get_mock_bin();
    let guard = spawn_gateway().await;
    let chan_a = test_channel("test/iso_a");
    let chan_b = test_channel("test/iso_b");

    let a = connect_client_with_retry(guard.socket()).await;
    let b = connect_client_with_retry(guard.socket()).await;
    attach_client(&a, &chan_a).await;
    attach_client(&b, &chan_b).await;
    Spawn::call(
        &*a,
        SpawnRequest {
            cmd: Some(vec![mock.clone(), "echo".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();
    Spawn::call(
        &*b,
        SpawnRequest {
            cmd: Some(vec![mock, "echo".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let (_, mut reader_b) = b.open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0).await.unwrap();
    let (writer_b, _) = b.open_channel(STREAM_INPUT_METHOD_ID, 0).await.unwrap();
    let (_, mut reader_a) = a.open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0).await.unwrap();
    let (writer_a, _) = a.open_channel(STREAM_INPUT_METHOD_ID, 0).await.unwrap();

    // Write ONLY to channel A; B's output must stay unpolluted.
    writer_a.send(b"hello-a\n".to_vec()).unwrap();
    let out_a = wait_for_output(&mut reader_a, b"hello-a", Duration::from_secs(3)).await;
    assert!(out_a.windows(7).any(|w| w == b"hello-a"));

    writer_b.send(b"hello-b\n".to_vec()).unwrap();
    let out_b = wait_for_output(&mut reader_b, b"hello-b", Duration::from_secs(3)).await;
    assert!(out_b.windows(7).any(|w| w == b"hello-b"));
    assert!(
        !out_b.windows(7).any(|w| w == b"hello-a"),
        "channel B output must not contain channel A's data"
    );
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn list_channels_reports_clients_and_geometry() {
    let mock = get_mock_bin();
    let guard = spawn_gateway().await;
    let channel = test_channel("test/list_geom");
    let c1 = connect_client_with_retry(guard.socket()).await;
    let c2 = connect_client_with_retry(guard.socket()).await;
    let _conn1 = attach_client(&c1, &channel).await;
    let _conn2 = attach_client(&c2, &channel).await;
    Spawn::call(
        &*c1,
        SpawnRequest {
            cmd: Some(vec![mock, "sleep".into(), "60000".into()]),
            cols: 120u16,
            rows: 40u16,
            cwd: None,
        },
    )
    .await
    .unwrap();
    Spawn::call(
        &*c2,
        SpawnRequest {
            cmd: None,
            cols: 80u16,
            rows: 24u16,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let resp = list_channels(&c1).await;
    let channels = &resp.channels;
    let ch = channels
        .iter()
        .find(|c| c.name == "test/list_geom")
        .unwrap();
    assert!(ch.session.is_some(), "session present");
    assert!(ch.clients.len() == 2, "two clients attached");
    // Geometry reflects the smallest client (80x24).
    assert_eq!(ch.session.as_ref().unwrap().cols, 80);
    assert_eq!(ch.session.as_ref().unwrap().rows, 24);
    // Each client carries its server-assigned conn_id, OS pid, hostname,
    // connect time, and physical terminal size.
    for cl in &ch.clients {
        assert!(cl.conn_id > 0, "conn_id is server-assigned");
        assert!(cl.pid > 0, "client OS pid recorded");
        assert!(!cl.hostname.is_empty(), "hostname recorded");
        assert!(cl.connected_at_unix > 0, "connect time recorded");
        // The client's declared physical size is at least the Spawn/Attach seed.
        assert!(cl.cols >= 80 && cl.rows >= 24, "physical size recorded");
    }
    // Channel reports its creation time.
    assert!(ch.created_at_unix > 0, "channel create time recorded");
    // Process visibility: the response identifies the daemon PID + socket.
    assert!(resp.gateway_pid > 0, "gateway pid reported");
    assert!(!resp.socket.is_empty(), "socket name reported");
    guard.shutdown().await;
}

/// Spawn a session running the mock in `spawn_child` mode on the client's
/// bound channel, read the grandchild PID it reports, and return it.
/// Panics if the marker never appears.
async fn spawn_session_with_grandchild(client: &Arc<RpcIpcClient>, mock: &str) -> u32 {
    Spawn::call(
        &**client,
        SpawnRequest {
            cmd: Some(vec![mock.to_string(), "spawn_child".into(), "60000".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .expect("spawn with grandchild");

    let (_, mut reader) = client
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .expect("subscribe output");

    const MARKER: &[u8] = b"GRANDCHILD_PID:";
    let output = wait_for_output(&mut reader, MARKER, Duration::from_secs(5)).await;
    let idx = output
        .windows(MARKER.len())
        .position(|w| w == MARKER)
        .unwrap_or_else(|| panic!("grandchild PID marker not found; got {output:?}"));
    let rest = &output[idx + MARKER.len()..];
    let pid_str: String = rest
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .map(|b| *b as char)
        .collect();
    pid_str
        .parse()
        .unwrap_or_else(|_| panic!("invalid grandchild PID in {rest:?}"))
}

/// Assert that after a kill RPC completes, the grandchild process dies too
/// (whole-tree containment, nothing orphaned). Polls briefly since
/// termination is asynchronous.
async fn assert_grandchild_dies(mock: &str, grandchild_pid: u32, via: &str) {
    let start = std::time::Instant::now();
    loop {
        if !mock_pid_alive(mock, grandchild_pid) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "grandchild {grandchild_pid} should be dead after {via}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// KillChannel must terminate the *entire* process tree — grandchildren
/// included, not just the PTY session leader. The mock's `spawn_child` mode
/// forks a grandchild and reports its PID on stdout; this test reads that
/// PID, confirms the grandchild is alive, kills the channel, then confirms
/// the grandchild died.
///
/// On Windows this exercises the Win32 Job Object containment
/// (`TerminateJobObject` on a `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` job); on
/// Unix it exercises the process-group SIGTERM→SIGKILL escalation.
#[tokio::test]
#[serial]
async fn kill_channel_terminates_process_tree() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/kill_tree")).await;
    let grandchild_pid = spawn_session_with_grandchild(&client, &mock).await;

    assert!(
        mock_pid_alive(&mock, grandchild_pid),
        "grandchild {grandchild_pid} should be alive before KillChannel"
    );

    KillChannel::call(&*client, ("test/kill_tree".to_string(), true))
        .await
        .unwrap();

    assert_grandchild_dies(&mock, grandchild_pid, "KillChannel").await;
    guard.shutdown().await;
}

/// CloseSession must terminate the *entire* process tree — grandchildren
/// included, not just the PTY session leader. Same containment guarantee as
/// KillChannel (both route through `request_session_kill` → `kill_child` →
/// `TerminateJobObject` on Windows / process-group kill on Unix), so the
/// grandchild must die here too.
#[tokio::test]
#[serial]
async fn close_session_terminates_process_tree() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/close_tree")).await;
    let grandchild_pid = spawn_session_with_grandchild(&client, &mock).await;

    assert!(
        mock_pid_alive(&mock, grandchild_pid),
        "grandchild {grandchild_pid} should be alive before CloseSession"
    );

    CloseSession::call(&*client, 1u64).await.unwrap();

    assert_grandchild_dies(&mock, grandchild_pid, "CloseSession").await;
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn kill_channel_kills_only_that_channel() {
    let mock = get_mock_bin();
    let guard = spawn_gateway().await;
    let chan_a = test_channel("test/kill_a");
    let chan_b = test_channel("test/kill_b");

    let a = connect_client_with_retry(guard.socket()).await;
    let b = connect_client_with_retry(guard.socket()).await;
    attach_client(&a, &chan_a).await;
    attach_client(&b, &chan_b).await;
    Spawn::call(
        &*a,
        SpawnRequest {
            cmd: Some(vec![mock.clone(), "sleep".into(), "60000".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();
    Spawn::call(
        &*b,
        SpawnRequest {
            cmd: Some(vec![mock, "echo".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let (_, mut reader_b) = b.open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0).await.unwrap();

    KillChannel::call(&*a, ("test/kill_a".to_string(), true))
        .await
        .unwrap();

    // Channel B must keep working.
    let (writer_b, _) = b.open_channel(STREAM_INPUT_METHOD_ID, 0).await.unwrap();
    writer_b.send(b"still-alive\n".to_vec()).unwrap();
    let out_b = wait_for_output(&mut reader_b, b"still-alive", Duration::from_secs(3)).await;
    assert!(out_b.windows(11).any(|w| w == b"still-alive"));

    // Channel A's session is gone (the channel may be reaped entirely once
    // its session exits and no clients remain — either way no live session).
    let channels = list_channels(&a).await;
    let ch_a = channels.channels.iter().find(|c| c.name == "test/kill_a");
    if let Some(ch) = ch_a {
        // If the channel survived reaping, its session must be gone.
        assert!(ch.session.is_none(), "killed channel has no session");
    }
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn kill_channel_respawns_with_stored_cmd() {
    let mock = get_mock_bin();
    let guard = spawn_gateway().await;
    let channel = test_channel("test/kill_respawn");
    let c = connect_client_with_retry(guard.socket()).await;
    attach_client(&c, &channel).await;
    Spawn::call(
        &*c,
        SpawnRequest {
            cmd: Some(vec![mock.clone(), "sleep".into(), "60000".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    KillChannel::call(&*c, ("test/kill_respawn".to_string(), true))
        .await
        .unwrap();

    // Re-attach (a fresh conn) and respawn — the stored cmd template is used.
    let c2 = connect_client_with_retry(guard.socket()).await;
    attach_client(&c2, &channel).await;

    // Wait for the kill to fully land before respawning: the killed session
    // lingers until the output-polling task observes its death, and a Spawn
    // during that window would "reuse" the dying session instead of respawning —
    // leaving the channel with no session right after respawn (CI-timing flake).
    let start = std::time::Instant::now();
    loop {
        let channels = list_channels(&c2).await;
        let ch = channels
            .channels
            .iter()
            .find(|c| c.name == "test/kill_respawn");
        let gone = ch.map(|c| c.session.is_none()).unwrap_or(true);
        if gone {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "killed session never cleared from channel"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let SpawnResponse {
        id,
        cols: _,
        rows: _,
    } = Spawn::call(
        &*c2,
        SpawnRequest {
            cmd: None,
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(id, 1);
    let channels = list_channels(&c2).await;
    let ch = channels
        .channels
        .iter()
        .find(|c| c.name == "test/kill_respawn")
        .unwrap();
    assert!(ch.session.is_some(), "session respawned");
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn kill_client_detaches_one_socket_only() {
    let mock = get_mock_bin();
    let guard = spawn_gateway().await;
    let channel = test_channel("test/kill_client");
    let c1 = connect_client_with_retry(guard.socket()).await;
    let c2 = connect_client_with_retry(guard.socket()).await;
    let admin = connect_client_with_retry(guard.socket()).await;
    let conn1 = attach_client(&c1, &channel).await;
    attach_client(&c2, &channel).await;
    Spawn::call(
        &*c1,
        SpawnRequest {
            cmd: Some(vec![mock, "echo".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();
    Spawn::call(
        &*c2,
        SpawnRequest {
            cmd: None,
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let (_, mut reader1) = c1
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();
    let (_, mut reader2) = c2
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();

    // Give the server's subscribe-registration tasks a moment to run before
    // the kill, so the eviction finds the subscriber (avoids a race where the
    // spawned subscribe task runs after KillClient evicts the conn).
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Use a separate admin connection so the RPC itself is not carried over
    // the connection being evicted.
    KillClient::call(&*admin, ("test/kill_client".to_string(), conn1))
        .await
        .unwrap();

    // Killed socket's stream ends.
    let mut got_end = false;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        match tokio::time::timeout(Duration::from_millis(500), reader1.recv()).await {
            Ok(None) => {
                got_end = true;
                break;
            }
            Ok(Some(Err(_))) => {
                got_end = true;
                break;
            }
            Ok(Some(_)) => continue,
            Err(_) => continue,
        }
    }
    assert!(got_end, "killed socket stream should end");

    // The other socket keeps working.
    let (writer2, _) = c2.open_channel(STREAM_INPUT_METHOD_ID, 0).await.unwrap();
    writer2.send(b"c2-alive\n".to_vec()).unwrap();
    let out2 = wait_for_output(&mut reader2, b"c2-alive", Duration::from_secs(3)).await;
    assert!(out2.windows(8).any(|w| w == b"c2-alive"));
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn kill_client_rejects_nonexistent_conn() {
    let guard = spawn_gateway().await;
    let channel = test_channel("test/kill_missing");
    let client = connect_client_with_retry(guard.socket()).await;
    attach_client(&client, &channel).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: None,
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    // A conn_id that was never assigned must be rejected, not silently
    // accepted (KillClient must target a real attached client).
    let err = KillClient::call(&*client, ("test/kill_missing".to_string(), 999_999))
        .await
        .expect_err("kill of a nonexistent conn must fail");
    assert!(
        err.to_string().contains("not attached"),
        "expected a 'not attached' error, got: {err}"
    );
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn kill_client_rejects_wrong_channel() {
    let guard = spawn_gateway().await;
    let channel_a = test_channel("test/kill_chan_a");
    let channel_b = test_channel("test/kill_chan_b");
    let a = connect_client_with_retry(guard.socket()).await;
    let b = connect_client_with_retry(guard.socket()).await;
    let conn_a = attach_client(&a, &channel_a).await;
    attach_client(&b, &channel_b).await;
    Spawn::call(
        &*a,
        SpawnRequest {
            cmd: None,
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();
    Spawn::call(
        &*b,
        SpawnRequest {
            cmd: None,
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    // conn_a is attached to channel_a; killing it "on channel_b" must fail and
    // must not detach it.
    let err = KillClient::call(&*b, ("test/kill_chan_b".to_string(), conn_a))
        .await
        .expect_err("kill of a client on the wrong channel must fail");
    assert!(
        err.to_string().contains("not attached"),
        "expected a 'not attached' error, got: {err}"
    );

    // The client is still attached (was not evicted by the rejected call).
    let resp = list_channels(&a).await;
    let ch = resp
        .channels
        .iter()
        .find(|c| c.name == "test/kill_chan_a")
        .unwrap();
    assert!(
        ch.clients.iter().any(|c| c.conn_id == conn_a),
        "client must remain attached after a rejected wrong-channel kill"
    );
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn unattached_client_rejected() {
    let guard = spawn_gateway().await;
    let client = connect_client_with_retry(guard.socket()).await;
    let result = Spawn::call(
        &*client,
        SpawnRequest {
            cmd: None,
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await;
    assert!(result.is_err(), "unattached Spawn must be rejected");
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn spawn_idempotent_on_live_session() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/spawn_idem")).await;
    let SpawnResponse {
        id: id1,
        cols: _,
        rows: _,
    } = Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock.clone(), "sleep".into(), "60000".into()]),
            cols: 120u16,
            rows: 40u16,
            cwd: None,
        },
    )
    .await
    .unwrap();
    // Second Spawn with a DIFFERENT cmd must reuse the live session.
    let SpawnResponse {
        id: id2,
        cols,
        rows,
    } = Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "exit".into(), "0".into()]),
            cols: 80u16,
            rows: 24u16,
            cwd: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(id1, id2, "same session id reused");
    assert_eq!(cols, 80, "geometry constrained to the smaller request");
    assert_eq!(rows, 24);
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn spawn_cmd_ignored_on_live_session() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/cmd_ignored")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock.clone(), "sleep".into(), "60000".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    // A second client joins the live session with a DIFFERENT cmd (`exit 0`).
    // If honored, the session would terminate immediately; instead it must be
    // ignored and the original `sleep` process kept running.
    let c2 = connect_client_with_retry(guard.socket()).await;
    attach_client(&c2, &test_channel("test/cmd_ignored")).await;
    let SpawnResponse {
        id: id2,
        cols: _,
        rows: _,
    } = Spawn::call(
        &*c2,
        SpawnRequest {
            cmd: Some(vec![mock, "exit".into(), "0".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    // Give a would-be `exit` time to take effect: the session must still be
    // live, proving the command was ignored and the original process survived.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let resp = list_channels(&client).await;
    let ch = resp
        .channels
        .iter()
        .find(|c| c.name == "test/cmd_ignored")
        .expect("channel listed");
    assert!(
        ch.session.as_ref().is_some_and(|s| !s.exited),
        "session must stay alive when a different cmd is supplied"
    );
    assert_eq!(id2, 1, "reused session id");
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn session_multi_client_pty_constrained_to_smallest() {
    let mock = get_mock_bin();
    let guard = spawn_gateway().await;
    let channel = test_channel("test/multi_client_smallest");

    let c1 = connect_client_with_retry(guard.socket()).await;
    let c2 = connect_client_with_retry(guard.socket()).await;
    let c3 = connect_client_with_retry(guard.socket()).await;
    attach_client(&c1, &channel).await;
    attach_client(&c2, &channel).await;
    attach_client(&c3, &channel).await;

    let SpawnResponse {
        id: pid1,
        cols: _,
        rows: _,
    } = Spawn::call(
        &*c1,
        SpawnRequest {
            cmd: Some(vec![mock.clone(), "sleep".into(), "60000".into()]),
            cols: 120u16,
            rows: 40u16,
            cwd: None,
        },
    )
    .await
    .unwrap();
    let SpawnResponse {
        id: _pid2,
        cols: _,
        rows: _,
    } = Spawn::call(
        &*c2,
        SpawnRequest {
            cmd: None,
            cols: 80u16,
            rows: 24u16,
            cwd: None,
        },
    )
    .await
    .unwrap();
    let SpawnResponse {
        id: _pid3,
        cols: _,
        rows: _,
    } = Spawn::call(
        &*c3,
        SpawnRequest {
            cmd: None,
            cols: 100u16,
            rows: 30u16,
            cwd: None,
        },
    )
    .await
    .unwrap();

    // All clients should see 80x24 (c2 is smallest)
    let start = std::time::Instant::now();
    loop {
        let (cols, rows) = ResizePty::call(&*c1, (pid1, 120u16, 40u16)).await.unwrap();
        if (cols, rows) == (80u16, 24u16) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "timed out waiting for 80x24"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn session_multi_client_disconnect_expands_pty() {
    let mock = get_mock_bin();
    let guard = spawn_gateway().await;
    let channel = test_channel("test/multi_client_expand");

    let c1 = connect_client_with_retry(guard.socket()).await;
    let c2 = connect_client_with_retry(guard.socket()).await;
    let c3 = connect_client_with_retry(guard.socket()).await;
    attach_client(&c1, &channel).await;
    attach_client(&c2, &channel).await;
    attach_client(&c3, &channel).await;

    let SpawnResponse {
        id: pid1,
        cols: _,
        rows: _,
    } = Spawn::call(
        &*c1,
        SpawnRequest {
            cmd: Some(vec![mock.clone(), "sleep".into(), "60000".into()]),
            cols: 120u16,
            rows: 40u16,
            cwd: None,
        },
    )
    .await
    .unwrap();
    let SpawnResponse {
        id: _pid2,
        cols: _,
        rows: _,
    } = Spawn::call(
        &*c2,
        SpawnRequest {
            cmd: None,
            cols: 80u16,
            rows: 24u16,
            cwd: None,
        },
    )
    .await
    .unwrap();
    let SpawnResponse {
        id: _pid3,
        cols: _,
        rows: _,
    } = Spawn::call(
        &*c3,
        SpawnRequest {
            cmd: None,
            cols: 100u16,
            rows: 30u16,
            cwd: None,
        },
    )
    .await
    .unwrap();

    // Drop c2 (smallest) → PTY expands to 100x30 (c3's size)
    drop(c2);
    let start = std::time::Instant::now();
    loop {
        let (cols, rows) = ResizePty::call(&*c1, (pid1, 120u16, 40u16)).await.unwrap();
        if (cols, rows) == (100u16, 30u16) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "timed out waiting for 100x30"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    drop(c3);
    let start = std::time::Instant::now();
    loop {
        let (cols, rows) = ResizePty::call(&*c1, (pid1, 120u16, 40u16)).await.unwrap();
        if (cols, rows) == (120u16, 40u16) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "timed out waiting for 120x40"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn shutdown_gateway_stops_daemon() {
    let mock = get_mock_bin();
    let guard = spawn_gateway().await;
    let channel = test_channel("test/shutdown");
    let client = connect_client_with_retry(guard.socket()).await;
    attach_client(&client, &channel).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "sleep".into(), "60000".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    guard.shutdown().await;

    // After the deferred flush grace, the gateway process ends; the tokio
    // task is aborted, so the session cannot be reached anymore. We simply
    // assert the ShutdownGateway call returned cleanly (the RPC response was
    // flushed before the daemon exited).
    // Give the daemon a moment to actually terminate.
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test]
#[serial]
async fn list_channels_orders_channels_and_clients_by_creation() {
    let mock = get_mock_bin();
    let guard = spawn_gateway().await;
    let socket = guard.socket().to_string();

    // Channel A is created before B; channel order must be [A, B], newest last.
    let ca = connect_client_with_retry(&socket).await;
    let cb = connect_client_with_retry(&socket).await;
    attach_client(&ca, &test_channel("test/order_a")).await;
    attach_client(&cb, &test_channel("test/order_b")).await;
    Spawn::call(
        &*ca,
        SpawnRequest {
            cmd: Some(vec![mock.clone(), "sleep".into(), "60000".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();
    Spawn::call(
        &*cb,
        SpawnRequest {
            cmd: Some(vec![mock, "sleep".into(), "60000".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    // A second client attaches to channel A AFTER B was created. Its conn_id
    // is higher, so it must sort after A's first client. (A client only shows
    // up in `list` once it has Spawned, so spawn it too.)
    let ca2 = connect_client_with_retry(&socket).await;
    attach_client(&ca2, &test_channel("test/order_a")).await;
    Spawn::call(
        &*ca2,
        SpawnRequest {
            cmd: None,
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let resp = list_channels(&ca).await;
    let names: Vec<&str> = resp.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        ["test/order_a", "test/order_b"],
        "channels must be in creation order, newest last: {names:?}"
    );

    let ch_a = resp
        .channels
        .iter()
        .find(|c| c.name == "test/order_a")
        .expect("channel a listed");
    assert_eq!(ch_a.clients.len(), 2, "two clients on channel a");
    assert!(
        ch_a.clients[0].conn_id < ch_a.clients[1].conn_id,
        "clients must be in connection order, newest last"
    );

    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn list_channels_reports_client_identity() {
    let guard = spawn_gateway().await;
    let channel = test_channel("test/client_identity");
    let client = connect_client_with_retry(guard.socket()).await;
    Attach::call(
        &*client,
        AttachRequest {
            channel: channel.to_string(),
            hostname: "host-a".to_string(),
            pid: 1234,
            user: "alice".to_string(),
            version: "9.9.9".to_string(),
            ssh_ip: Some("192.168.1.50".to_string()),
        },
    )
    .await
    .unwrap();
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: None,
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    let resp = list_channels(&client).await;
    let ch = resp
        .channels
        .iter()
        .find(|c| c.name == "test/client_identity")
        .expect("channel listed");
    assert_eq!(ch.clients.len(), 1);
    let c = &ch.clients[0];
    assert_eq!(
        c.user, "alice",
        "user reported at Attach must surface in list"
    );
    assert_eq!(
        c.version, "9.9.9",
        "version reported at Attach must surface in list"
    );
    assert_eq!(
        c.ssh_ip.as_deref(),
        Some("192.168.1.50"),
        "ssh_ip reported at Attach must surface in list"
    );
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn shutdown_refuses_without_force_when_live_sessions() {
    let mock = get_mock_bin();
    let guard = spawn_gateway().await;
    let client = connect_client_with_retry(guard.socket()).await;
    attach_client(&client, &test_channel("test/force")).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "sleep".into(), "60000".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    // Without force: refused, and the gateway must stay fully operational.
    let err = ShutdownGateway::call(&*client, false)
        .await
        .expect_err("stop without force while a live session runs must fail");
    assert!(
        err.to_string().contains("live session"),
        "refusal message should mention live sessions, got: {err}"
    );

    // Still reachable after the refusal.
    let resp = list_channels(&client).await;
    assert_eq!(
        resp.channels.len(),
        1,
        "gateway still serving after refusal"
    );

    // With force: clean shutdown.
    ShutdownGateway::call(&*client, true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test]
#[serial]
async fn shutdown_without_force_succeeds_when_no_live_sessions() {
    let guard = spawn_gateway().await;
    let client = connect_client_with_retry(guard.socket()).await;
    // No sessions were ever spawned: stop must succeed without --force.
    ShutdownGateway::call(&*client, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test]
#[serial]
async fn kill_channel_refuses_without_force_when_participants() {
    let mock = get_mock_bin();
    let guard = spawn_gateway().await;
    let channel = test_channel("test/kill_force");
    let client = connect_client_with_retry(guard.socket()).await;
    attach_client(&client, &channel).await;
    Spawn::call(
        &*client,
        SpawnRequest {
            cmd: Some(vec![mock, "sleep".into(), "60000".into()]),
            cols: TEST_COLS,
            rows: TEST_ROWS,
            cwd: None,
        },
    )
    .await
    .unwrap();

    // Without force: refused, and the channel must stay fully operational.
    let err = KillChannel::call(&*client, (channel.to_string(), false))
        .await
        .expect_err("kill without force while participants are attached must fail");
    assert!(
        err.to_string().contains("participant"),
        "refusal message should mention participants, got: {err}"
    );

    // Still reachable after the refusal, session still alive.
    let resp = list_channels(&client).await;
    let ch = resp
        .channels
        .iter()
        .find(|c| c.name == channel.to_string())
        .expect("channel still served after refusal");
    assert!(
        ch.session.as_ref().is_some_and(|s| !s.exited),
        "session must survive a refused kill"
    );

    // With force: the channel is killed.
    KillChannel::call(&*client, (channel.to_string(), true))
        .await
        .unwrap();
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn kill_channel_without_force_succeeds_when_no_participants() {
    let guard = spawn_gateway().await;
    let client = connect_client_with_retry(guard.socket()).await;
    // A non-existent channel has no participants: kill succeeds without force.
    KillChannel::call(&*client, ("test/nonexistent".to_string(), false))
        .await
        .unwrap();
    guard.shutdown().await;
}

/// Workspace switching is driven by `RebindWorkspace`: the outer viewer asks
/// the gateway to rebind every viewer attached to `source_channel` to a target
/// channel. The server must push `OnWorkspaceRebind { target }` to the
/// attached viewer, which the outer launcher uses to reconnect (the
/// `Ok(Some(target))` return path of `run_session`).
#[tokio::test]
async fn rebind_workspace_pushes_target_to_attached_viewer() {
    use muxio_rpc_service_endpoint::RpcServiceEndpointInterface;
    use term_session_muxio_service_definitions::{
        OnWorkspaceRebind, RebindWorkspace, RebindWorkspaceRequest,
    };

    let source = test_channel("test/rebind-src");
    let (client, _conn_id, guard) = spawn_session(&source).await;

    let (target_tx, mut target_rx) = tokio::sync::mpsc::unbounded_channel();
    {
        let target_tx = target_tx.clone();
        client
            .get_endpoint()
            .register_prebuffered(OnWorkspaceRebind::METHOD_ID, move |payload, _ctx| {
                let target_tx = target_tx.clone();
                async move {
                    let req = OnWorkspaceRebind::decode_request(&payload)
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                    let _ = target_tx.send(req.target);
                    OnWorkspaceRebind::encode_response(())
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                }
            })
            .await
            .expect("register OnWorkspaceRebind handler");
    }

    RebindWorkspace::call(
        &*client,
        RebindWorkspaceRequest { scope: term_session_muxio_service_definitions::RebindScope::CallerOnly, initiator_conn_id: None, 
            source_channel: source.to_string(),
            target: "ws-123/main".to_string(),
        },
    )
    .await
    .expect("rebind workspace");

    let received = tokio::time::timeout(Duration::from_secs(2), target_rx.recv())
        .await
        .expect("timed out waiting for OnWorkspaceRebind push")
        .expect("target channel closed");
    assert_eq!(received, "ws-123/main");

    guard.shutdown().await;
}

/// `RebindWorkspace` with an unknown source channel is a no-op: no viewer is
/// attached there, so no `OnWorkspaceRebind` push may arrive.
#[tokio::test]
async fn rebind_workspace_to_unknown_source_sends_no_push() {
    use muxio_rpc_service_endpoint::RpcServiceEndpointInterface;
    use term_session_muxio_service_definitions::{
        OnWorkspaceRebind, RebindWorkspace, RebindWorkspaceRequest,
    };

    let source = test_channel("test/rebind-unknown");
    let (client, _conn_id, guard) = spawn_session(&source).await;

    let (target_tx, mut target_rx) = tokio::sync::mpsc::unbounded_channel();
    {
        let target_tx = target_tx.clone();
        client
            .get_endpoint()
            .register_prebuffered(OnWorkspaceRebind::METHOD_ID, move |payload, _ctx| {
                let target_tx = target_tx.clone();
                async move {
                    let req = OnWorkspaceRebind::decode_request(&payload)
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                    let _ = target_tx.send(req.target);
                    OnWorkspaceRebind::encode_response(())
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                }
            })
            .await
            .expect("register OnWorkspaceRebind handler");
    }

    // No connection is attached to this source channel.
    RebindWorkspace::call(
        &*client,
        RebindWorkspaceRequest { scope: term_session_muxio_service_definitions::RebindScope::CallerOnly, initiator_conn_id: None, 
            source_channel: "test/rebind-nobody".to_string(),
            target: "ws-123/main".to_string(),
        },
    )
    .await
    .expect("rebind to unknown source must still succeed");

    let maybe = tokio::time::timeout(Duration::from_millis(500), target_rx.recv()).await;
    assert!(
        maybe.is_err(),
        "no OnWorkspaceRebind push expected for an unknown source channel"
    );

    guard.shutdown().await;
}

/// The term-wm launcher must exit immediately (no retry loop) when the nesting
/// guard fires. Spawns `term-wm` inside an environment where
/// `TERM_SESSION_GATEWAY` matches `TERM_WM_GATEWAY` (same-gateway inception),
/// and asserts that the process exits non-zero with the FATAL diagnostic
/// within a short timeout — proving the retry loop was never entered.
#[test]
fn launcher_exits_immediately_on_nesting_fatal() {
    use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};
    use std::process::{Command, Stdio};
    use std::time::Instant;

    // Use a 2-segment gateway so ChannelName::parse succeeds and both
    // `connect_or_spawn_server` (which resolves via gateway_channel_name())
    // and `TERM_SESSION_GATEWAY` (read literally by the nesting guard) agree.
    let gateway = "test-nesting/launcher-gw";

    // Bind a dummy listener so connect_or_spawn_server's probe succeeds
    // and returns immediately (instead of trying to spawn a daemon).
    let name = gateway
        .to_ns_name::<GenericNamespaced>()
        .expect("gateway ns name");
    let _listener = ListenerOptions::new()
        .name(name)
        .try_overwrite(true)
        .create_sync()
        .expect("bind dummy gateway");

    let mut child = Command::new(env!("CARGO_BIN_EXE_term-wm"))
        .env("TERM_WM_GATEWAY", gateway)
        .env("TERM_SESSION_GATEWAY", gateway)
        .args(["--workspace", "test-nesting"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn term-wm");

    let start = Instant::now();
    loop {
        if start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            panic!(
                "term-wm did not exit within 10s — likely retrying instead of exiting immediately"
            );
        }
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let elapsed = start.elapsed();

    let out = child.wait_with_output().expect("wait");
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !out.status.success(),
        "term-wm must exit non-zero on same-gateway nesting"
    );
    assert!(
        stderr.contains("FATAL"),
        "must print nesting FATAL diagnostic, got stderr: {stderr}"
    );
    assert!(
        stderr.contains("term-wm"),
        "error must mention term-wm, not term-session: {stderr}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "term-wm took {elapsed:?} — likely retrying instead of exiting immediately"
    );
}
