use muxio_tokio_mpsc_adapter::ChannelCallerExt;
use muxio_tokio_rpc_ipc_client::RpcCallPrebuffered;
use serial_test::serial;
use std::time::Duration;
use term_session_muxio_service_definitions::{
    CloseSession, ListSessions, ResizePty, STREAM_INPUT_METHOD_ID, SUBSCRIBE_OUTPUT_METHOD_ID,
    Spawn,
};

mod common;
use common::mock::{find_osc52_payload, find_sgr_mouse_token, get_mock_bin};
use common::session::{
    TEST_COLS, TEST_ROWS, connect_client_with_retry, get_bench_bin, spawn_session, test_channel,
    wait_for_output,
};
use term_wm_pty_engine::clipboard::Osc52Extractor;

#[tokio::test]
async fn session_spawn_returns_id() {
    let mock = get_mock_bin();
    let client = spawn_session(
        &test_channel("test/spawn_returns_id"),
        vec![mock, "echo".into()],
    )
    .await;
    let (id, _, _) = Spawn::call(&*client, (None, TEST_COLS, TEST_ROWS))
        .await
        .unwrap();
    assert_eq!(id, 1);
}

#[tokio::test]
async fn session_input_output_roundtrip() {
    let mock = get_mock_bin();
    let client = spawn_session(
        &test_channel("test/input_output"),
        vec![mock, "echo".into()],
    )
    .await;

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
}

#[cfg(not(windows))]
#[tokio::test]
async fn session_mouse_bytes_forwarded() {
    let mock = get_mock_bin();
    let client = spawn_session(
        &test_channel("test/mouse_forward"),
        vec![mock, "echo".into()],
    )
    .await;

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
}

/// On Windows, ConPTY intercepts escape sequences written to the PTY master's
/// stdin pipe before they reach the child process. The `capture` subcommand
/// verifies the PTY input→output pipeline using a `MOUSE_OK:` sentinel marker
/// instead of raw escape sequences.
#[cfg(windows)]
#[tokio::test]
async fn session_mouse_bytes_forwarded() {
    let mock = get_mock_bin();
    let client = spawn_session(
        &test_channel("test/mouse_forward"),
        vec![mock, "capture".into()],
    )
    .await;

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
}

#[tokio::test]
#[serial]
async fn session_osc52_in_output() {
    let mock = get_mock_bin();
    let client = spawn_session(
        &test_channel("test/osc52_output"),
        vec![mock, "osc52".into()],
    )
    .await;

    let (_, mut reader) = client
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();

    let output = wait_for_output(&mut reader, b"52;", Duration::from_secs(3)).await;
    let payload = find_osc52_payload(&output);
    assert_eq!(
        payload,
        Some(common::mock::EXPECTED_OSC52_PAYLOAD),
        "OSC 52 payload extraction failed, got stream: {:?}",
        String::from_utf8_lossy(&output)
    );
}

// Note: [serial] was added due to some Windows flakiness
#[tokio::test]
#[serial]
async fn session_osc52_via_osc52extractor() {
    let mock = get_mock_bin();
    let client = spawn_session(
        &test_channel("test/osc52_extractor"),
        vec![mock, "osc52".into()],
    )
    .await;

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
                eprintln!(
                    "DEBUG chunk len={} is_active={} hex={:02x?} text={:?}",
                    data.len(),
                    extractor.is_active(),
                    &data[..data.len().min(40)],
                    String::from_utf8_lossy(&data[..data.len().min(80)])
                );
                if let Some(text) = extractor.push(&data, &prev_tail) {
                    eprintln!("DEBUG EXTRACTED {:?}", text);
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
    eprintln!(
        "DEBUG final extracted={:?} is_active={}",
        extracted,
        extractor.is_active()
    );

    assert_eq!(
        extracted,
        Some("test".to_string()),
        "Osc52Extractor should decode 'test' from real server byte stream"
    );
}

#[tokio::test]
async fn session_resize() {
    let mock = get_mock_bin();
    let client = spawn_session(&test_channel("test/resize"), vec![mock, "echo".into()]).await;

    let result = ResizePty::call(&*client, (1u64, 120u16, 40u16)).await;
    assert!(result.is_ok(), "Resize should succeed: {:?}", result.err());
}

#[tokio::test]
async fn session_list_sessions() {
    let mock = get_mock_bin();
    let client = spawn_session(
        &test_channel("test/list_sessions"),
        vec![mock, "sleep".into(), "60000".into()],
    )
    .await;

    let sessions = ListSessions::call(&*client, ()).await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].0, 1);
}

#[tokio::test]
async fn session_close_session() {
    let mock = get_mock_bin();
    let client = spawn_session(
        &test_channel("test/close_session"),
        vec![mock, "sleep".into(), "60000".into()],
    )
    .await;

    CloseSession::call(&*client, 1u64).await.unwrap();

    let sessions = ListSessions::call(&*client, ()).await.unwrap();
    assert!(sessions.is_empty(), "Session should be removed after close");
}

// Note: [serial] was added due to some Windows flakiness
#[tokio::test]
#[serial]
async fn session_child_exit() {
    let mock = get_mock_bin();
    let client = spawn_session(
        &test_channel("test/child_exit"),
        vec![mock, "exit".into(), "0".into()],
    )
    .await;

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
}

// Note: [serial] was added due to some Windows flakiness
#[tokio::test]
#[serial]
async fn session_child_exit_before_subscribe() {
    let mock = get_mock_bin();
    let client = spawn_session(
        &test_channel("test/child_exit_early"),
        vec![mock, "exit".into(), "0".into()],
    )
    .await;

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
}

#[tokio::test]
async fn session_reconnect() {
    let mock = get_mock_bin();
    let channel = test_channel("test/reconnect");
    let config = term_session_server::SessionServerConfig {
        channel: channel.clone(),
        cmd: vec![mock, "echo".into()],
    };
    tokio::spawn(async move { term_session_server::run_server(config).await });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client1 = connect_client_with_retry(&channel.to_string()).await;
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

    let client2 = connect_client_with_retry(&channel.to_string()).await;
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
}

#[tokio::test]
async fn term_bench_runs_to_completion() {
    let bench_bin = get_bench_bin();
    if !bench_bin.exists() {
        eprintln!("Skipping term_bench test: binary not found at {bench_bin:?}");
        return;
    }

    let client = spawn_session(
        &test_channel("test/bench"),
        vec![
            bench_bin.to_string_lossy().to_string(),
            "-d".into(),
            "1".into(),
            "-f".into(),
            "10".into(),
        ],
    )
    .await;

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
async fn session_multi_client_pty_constrained_to_smallest() {
    let mock = get_mock_bin();
    let channel = test_channel("test/multi_client_smallest");
    let config = term_session_server::SessionServerConfig {
        channel: channel.clone(),
        cmd: vec![mock, "echo".into()],
    };
    tokio::spawn(async move { term_session_server::run_server(config).await });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let c1 = connect_client_with_retry(&channel.to_string()).await;
    let c2 = connect_client_with_retry(&channel.to_string()).await;
    let c3 = connect_client_with_retry(&channel.to_string()).await;

    let (pid1, _, _) = Spawn::call(&*c1, (None, 120u16, 40u16)).await.unwrap();
    let (pid2, _, _) = Spawn::call(&*c2, (None, 80u16, 24u16)).await.unwrap();
    let (pid3, _, _) = Spawn::call(&*c3, (None, 100u16, 30u16)).await.unwrap();

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
    let start = std::time::Instant::now();
    loop {
        let (cols, rows) = ResizePty::call(&*c2, (pid2, 80u16, 24u16)).await.unwrap();
        if (cols, rows) == (80u16, 24u16) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "timed out waiting for 80x24"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let start = std::time::Instant::now();
    loop {
        let (cols, rows) = ResizePty::call(&*c3, (pid3, 100u16, 30u16)).await.unwrap();
        if (cols, rows) == (80u16, 24u16) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "timed out waiting for 80x24"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
#[serial]
async fn session_multi_client_disconnect_expands_pty() {
    let mock = get_mock_bin();
    let channel = test_channel("test/multi_client_expand");
    let config = term_session_server::SessionServerConfig {
        channel: channel.clone(),
        cmd: vec![mock, "echo".into()],
    };
    tokio::spawn(async move { term_session_server::run_server(config).await });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let c1 = connect_client_with_retry(&channel.to_string()).await;
    let c2 = connect_client_with_retry(&channel.to_string()).await;
    let c3 = connect_client_with_retry(&channel.to_string()).await;

    let (pid1, _, _) = Spawn::call(&*c1, (None, 120u16, 40u16)).await.unwrap();
    let (_pid2, _, _) = Spawn::call(&*c2, (None, 80u16, 24u16)).await.unwrap();
    let (_pid3, _, _) = Spawn::call(&*c3, (None, 100u16, 30u16)).await.unwrap();

    // Verify constrained to 80x24
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

    // Drop c3 → only c1 remains, PTY expands to 120x40
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
}

#[tokio::test]
#[serial]
async fn session_server_start_stop_cleanly() {
    let mock = get_mock_bin();
    let channel = test_channel("test/server_start_stop");
    let config = term_session_server::SessionServerConfig {
        channel: channel.clone(),
        cmd: vec![mock, "echo".into()],
    };
    tokio::spawn(async move { term_session_server::run_server(config).await });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = connect_client_with_retry(&channel.to_string()).await;
    let sessions = ListSessions::call(&*client, ()).await.unwrap();
    assert_eq!(sessions.len(), 1);
}
