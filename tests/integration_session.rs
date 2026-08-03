use muxio_tokio_mpsc_adapter::ChannelCallerExt;
use muxio_tokio_rpc_ipc_client::{RpcCallPrebuffered, RpcIpcClient};
use serial_test::serial;
use std::sync::Arc;
use std::time::Duration;
use term_session_muxio_service_definitions::{
    CloseSession, KillChannel, KillClient, ResizePty, STREAM_INPUT_METHOD_ID,
    SUBSCRIBE_OUTPUT_METHOD_ID, ShutdownGateway, Spawn,
};

mod common;
use common::mock::{find_osc52_payload, find_sgr_mouse_token, get_mock_bin, mock_pid_alive};
use common::session::{
    TEST_COLS, TEST_ROWS, attach_client, connect_client_with_retry, get_bench_bin, list_channels,
    spawn_gateway, spawn_session, test_channel, wait_for_output,
};
use term_wm_pty_engine::clipboard::Osc52Extractor;

#[tokio::test]
async fn session_spawn_returns_id() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/spawn_returns_id")).await;
    let (id, _, _) = Spawn::call(
        &*client,
        (Some(vec![mock, "echo".into()]), TEST_COLS, TEST_ROWS),
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
        (Some(vec![mock, "echo".into()]), TEST_COLS, TEST_ROWS),
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

#[cfg(not(windows))]
#[tokio::test]
async fn session_mouse_bytes_forwarded() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/mouse_forward")).await;
    Spawn::call(
        &*client,
        (Some(vec![mock, "echo".into()]), TEST_COLS, TEST_ROWS),
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
        (Some(vec![mock, "capture".into()]), TEST_COLS, TEST_ROWS),
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
        (Some(vec![mock, "osc52".into()]), TEST_COLS, TEST_ROWS),
    )
    .await
    .unwrap();

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
        (Some(vec![mock, "osc52".into()]), TEST_COLS, TEST_ROWS),
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

#[tokio::test]
async fn session_resize() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/resize")).await;
    Spawn::call(
        &*client,
        (Some(vec![mock, "echo".into()]), TEST_COLS, TEST_ROWS),
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
        (
            Some(vec![mock, "sleep".into(), "60000".into()]),
            TEST_COLS,
            TEST_ROWS,
        ),
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
        (
            Some(vec![mock, "sleep".into(), "60000".into()]),
            TEST_COLS,
            TEST_ROWS,
        ),
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
        (
            Some(vec![mock, "exit".into(), "0".into()]),
            TEST_COLS,
            TEST_ROWS,
        ),
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
        (
            Some(vec![mock, "exit".into(), "0".into()]),
            TEST_COLS,
            TEST_ROWS,
        ),
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
        (
            Some(vec![mock.clone(), "echo".into()]),
            TEST_COLS,
            TEST_ROWS,
        ),
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
        (
            Some(vec![
                bench_bin.to_string_lossy().to_string(),
                "-d".into(),
                "1".into(),
                "-f".into(),
                "10".into(),
            ]),
            TEST_COLS,
            TEST_ROWS,
        ),
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
        (
            Some(vec![mock.clone(), "echo".into()]),
            TEST_COLS,
            TEST_ROWS,
        ),
    )
    .await
    .unwrap();
    Spawn::call(&*b, (Some(vec![mock, "echo".into()]), TEST_COLS, TEST_ROWS))
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
        (
            Some(vec![mock, "sleep".into(), "60000".into()]),
            120u16,
            40u16,
        ),
    )
    .await
    .unwrap();
    Spawn::call(&*c2, (None, 80u16, 24u16)).await.unwrap();

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
        (
            Some(vec![mock.to_string(), "spawn_child".into(), "60000".into()]),
            TEST_COLS,
            TEST_ROWS,
        ),
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

    KillChannel::call(&*client, "test/kill_tree".to_string())
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
        (
            Some(vec![mock.clone(), "sleep".into(), "60000".into()]),
            TEST_COLS,
            TEST_ROWS,
        ),
    )
    .await
    .unwrap();
    Spawn::call(&*b, (Some(vec![mock, "echo".into()]), TEST_COLS, TEST_ROWS))
        .await
        .unwrap();

    let (_, mut reader_b) = b.open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0).await.unwrap();

    KillChannel::call(&*a, "test/kill_a".to_string())
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
        (
            Some(vec![mock.clone(), "sleep".into(), "60000".into()]),
            TEST_COLS,
            TEST_ROWS,
        ),
    )
    .await
    .unwrap();

    KillChannel::call(&*c, "test/kill_respawn".to_string())
        .await
        .unwrap();

    // Re-attach (a fresh conn) and respawn — the stored cmd template is used.
    let c2 = connect_client_with_retry(guard.socket()).await;
    attach_client(&c2, &channel).await;
    let (id, _, _) = Spawn::call(&*c2, (None, TEST_COLS, TEST_ROWS))
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
        (Some(vec![mock, "echo".into()]), TEST_COLS, TEST_ROWS),
    )
    .await
    .unwrap();
    Spawn::call(&*c2, (None, TEST_COLS, TEST_ROWS))
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
    Spawn::call(&*client, (None, TEST_COLS, TEST_ROWS))
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
    Spawn::call(&*a, (None, TEST_COLS, TEST_ROWS))
        .await
        .unwrap();
    Spawn::call(&*b, (None, TEST_COLS, TEST_ROWS))
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
    let result = Spawn::call(&*client, (None, TEST_COLS, TEST_ROWS)).await;
    assert!(result.is_err(), "unattached Spawn must be rejected");
    guard.shutdown().await;
}

#[tokio::test]
#[serial]
async fn spawn_idempotent_on_live_session() {
    let mock = get_mock_bin();
    let (client, _conn_id, guard) = spawn_session(&test_channel("test/spawn_idem")).await;
    let (id1, _, _) = Spawn::call(
        &*client,
        (
            Some(vec![mock.clone(), "sleep".into(), "60000".into()]),
            120u16,
            40u16,
        ),
    )
    .await
    .unwrap();
    // Second Spawn with a DIFFERENT cmd must reuse the live session.
    let (id2, cols, rows) = Spawn::call(
        &*client,
        (Some(vec![mock, "exit".into(), "0".into()]), 80u16, 24u16),
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
        (
            Some(vec![mock.clone(), "sleep".into(), "60000".into()]),
            TEST_COLS,
            TEST_ROWS,
        ),
    )
    .await
    .unwrap();

    // A second client joins the live session with a DIFFERENT cmd (`exit 0`).
    // If honored, the session would terminate immediately; instead it must be
    // ignored and the original `sleep` process kept running.
    let c2 = connect_client_with_retry(guard.socket()).await;
    attach_client(&c2, &test_channel("test/cmd_ignored")).await;
    let (id2, _, _) = Spawn::call(
        &*c2,
        (Some(vec![mock, "exit".into(), "0".into()]), TEST_COLS, TEST_ROWS),
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

    let (pid1, _, _) = Spawn::call(
        &*c1,
        (
            Some(vec![mock.clone(), "sleep".into(), "60000".into()]),
            120u16,
            40u16,
        ),
    )
    .await
    .unwrap();
    let (_pid2, _, _) = Spawn::call(&*c2, (None, 80u16, 24u16)).await.unwrap();
    let (_pid3, _, _) = Spawn::call(&*c3, (None, 100u16, 30u16)).await.unwrap();

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

    let (pid1, _, _) = Spawn::call(
        &*c1,
        (
            Some(vec![mock.clone(), "sleep".into(), "60000".into()]),
            120u16,
            40u16,
        ),
    )
    .await
    .unwrap();
    let (_pid2, _, _) = Spawn::call(&*c2, (None, 80u16, 24u16)).await.unwrap();
    let (_pid3, _, _) = Spawn::call(&*c3, (None, 100u16, 30u16)).await.unwrap();

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
        (
            Some(vec![mock, "sleep".into(), "60000".into()]),
            TEST_COLS,
            TEST_ROWS,
        ),
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
        (
            Some(vec![mock.clone(), "sleep".into(), "60000".into()]),
            TEST_COLS,
            TEST_ROWS,
        ),
    )
    .await
    .unwrap();
    Spawn::call(
        &*cb,
        (Some(vec![mock, "sleep".into(), "60000".into()]), TEST_COLS, TEST_ROWS),
    )
    .await
    .unwrap();

    // A second client attaches to channel A AFTER B was created. Its conn_id
    // is higher, so it must sort after A's first client. (A client only shows
    // up in `list` once it has Spawned, so spawn it too.)
    let ca2 = connect_client_with_retry(&socket).await;
    attach_client(&ca2, &test_channel("test/order_a")).await;
    Spawn::call(&*ca2, (None, TEST_COLS, TEST_ROWS)).await.unwrap();

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
async fn shutdown_refuses_without_force_when_live_sessions() {
    let mock = get_mock_bin();
    let guard = spawn_gateway().await;
    let client = connect_client_with_retry(guard.socket()).await;
    attach_client(&client, &test_channel("test/force")).await;
    Spawn::call(
        &*client,
        (
            Some(vec![mock, "sleep".into(), "60000".into()]),
            TEST_COLS,
            TEST_ROWS,
        ),
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
    assert_eq!(resp.channels.len(), 1, "gateway still serving after refusal");

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
