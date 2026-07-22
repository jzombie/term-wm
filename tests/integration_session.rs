use muxio_tokio_mpsc_adapter::ChannelCallerExt;
use muxio_tokio_rpc_ipc_client::RpcCallPrebuffered;
use std::time::Duration;
use term_session_muxio_service_definitions::{
    CloseSession, ListSessions, ResizePty, STREAM_INPUT_METHOD_ID, SUBSCRIBE_OUTPUT_METHOD_ID,
    Spawn,
};

mod common;
use common::mock::{find_osc52_payload, find_sgr_mouse_token, get_mock_bin};
use common::session::{
    TEST_COLS, TEST_ROWS, connect_client_with_retry, generate_socket_path, get_bench_bin,
    spawn_session, wait_for_output,
};
use term_wm_pty_engine::clipboard::Osc52Extractor;

#[tokio::test]
async fn session_spawn_returns_id() {
    let mock = get_mock_bin();
    let (client, _dir) = spawn_session(vec![mock, "echo".into()], TEST_COLS, TEST_ROWS).await;
    let id = Spawn::call(&*client, (None, TEST_COLS, TEST_ROWS))
        .await
        .unwrap();
    assert_eq!(id, 1);
}

#[tokio::test]
async fn session_input_output_roundtrip() {
    let mock = get_mock_bin();
    let (client, _dir) = spawn_session(vec![mock, "echo".into()], TEST_COLS, TEST_ROWS).await;

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
    let (client, _dir) = spawn_session(vec![mock, "echo".into()], TEST_COLS, TEST_ROWS).await;

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
    let (client, _dir) = spawn_session(vec![mock, "capture".into()], TEST_COLS, TEST_ROWS).await;

    let (_, mut reader) = client
        .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
        .await
        .unwrap();
    let (writer, _) = client
        .open_channel(STREAM_INPUT_METHOD_ID, 0)
        .await
        .unwrap();

    writer.send(b"ping".to_vec()).unwrap();

    let output = wait_for_output(&mut reader, b"MOUSE_OK:", Duration::from_secs(3)).await;
    assert!(
        output.windows(9).any(|w| w == b"MOUSE_OK:"),
        "PTY input pipeline broken on Windows, got {} bytes: {:?}",
        output.len(),
        String::from_utf8_lossy(&output)
    );
}

#[tokio::test]
async fn session_osc52_in_output() {
    let mock = get_mock_bin();
    let (client, _dir) = spawn_session(vec![mock, "osc52".into()], TEST_COLS, TEST_ROWS).await;

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

#[tokio::test]
async fn session_osc52_via_osc52extractor() {
    let mock = get_mock_bin();
    let (client, _dir) = spawn_session(vec![mock, "osc52".into()], TEST_COLS, TEST_ROWS).await;

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

    assert_eq!(
        extracted,
        Some("test".to_string()),
        "Osc52Extractor should decode 'test' from real server byte stream"
    );
}

#[tokio::test]
async fn session_resize() {
    let mock = get_mock_bin();
    let (client, _dir) = spawn_session(vec![mock, "echo".into()], TEST_COLS, TEST_ROWS).await;

    let result = ResizePty::call(&*client, (1u64, 120u16, 40u16)).await;
    assert!(result.is_ok(), "Resize should succeed: {:?}", result.err());
}

#[tokio::test]
async fn session_list_sessions() {
    let mock = get_mock_bin();
    let (client, _dir) = spawn_session(
        vec![mock, "sleep".into(), "60000".into()],
        TEST_COLS,
        TEST_ROWS,
    )
    .await;

    let sessions = ListSessions::call(&*client, ()).await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].0, 1);
}

#[tokio::test]
async fn session_close_session() {
    let mock = get_mock_bin();
    let (client, _dir) = spawn_session(
        vec![mock, "sleep".into(), "60000".into()],
        TEST_COLS,
        TEST_ROWS,
    )
    .await;

    CloseSession::call(&*client, 1u64).await.unwrap();

    let sessions = ListSessions::call(&*client, ()).await.unwrap();
    assert!(sessions.is_empty(), "Session should be removed after close");
}

#[tokio::test]
async fn session_child_exit() {
    let mock = get_mock_bin();
    let (client, _dir) =
        spawn_session(vec![mock, "exit".into(), "0".into()], TEST_COLS, TEST_ROWS).await;

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

#[tokio::test]
async fn session_reconnect() {
    let mock = get_mock_bin();
    let (tempdir, socket_path) = generate_socket_path();
    let config = term_session_server::SessionServerConfig {
        socket_path: socket_path.clone(),
        cmd: vec![mock, "echo".into()],
        cols: TEST_COLS,
        rows: TEST_ROWS,
    };
    tokio::spawn(async move { term_session_server::run_server(config).await });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client1 = connect_client_with_retry(&socket_path).await;
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

    let client2 = connect_client_with_retry(&socket_path).await;
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

    drop(tempdir);
}

#[tokio::test]
async fn term_bench_runs_to_completion() {
    let bench_bin = get_bench_bin();
    if !bench_bin.exists() {
        eprintln!("Skipping term_bench test: binary not found at {bench_bin:?}");
        return;
    }

    let (client, _dir) = spawn_session(
        vec![
            bench_bin.to_string_lossy().to_string(),
            "-d".into(),
            "1".into(),
            "-f".into(),
            "10".into(),
        ],
        TEST_COLS,
        TEST_ROWS,
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
