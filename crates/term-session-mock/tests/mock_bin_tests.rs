use std::process::Command;
use std::time::Duration;

fn mock_bin() -> String {
    let mut path = std::env::current_exe().expect("test exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(format!("term-session-mock{}", std::env::consts::EXE_SUFFIX));
    path.to_string_lossy().to_string()
}

fn find_osc52_payload(stream: &[u8]) -> Option<&[u8]> {
    let standard_prefix = b"\x1b]52;";
    let windows_prefix = "←]52;".as_bytes();
    let (start_idx, prefix_len) = stream
        .windows(standard_prefix.len())
        .position(|w| w == standard_prefix)
        .map(|idx| (idx, standard_prefix.len()))
        .or_else(|| {
            stream
                .windows(windows_prefix.len())
                .position(|w| w == windows_prefix)
                .map(|idx| (idx, windows_prefix.len()))
        })?;
    let body_start = start_idx + prefix_len;
    let mut body_end = body_start;
    while body_end < stream.len() {
        let b = stream[body_end];
        if b == 0x07 || (b == 0x1b && body_end + 1 < stream.len() && stream[body_end + 1] == b'\\')
        {
            break;
        }
        let is_valid_body =
            b.is_ascii_alphanumeric() || b == b';' || b == b'+' || b == b'/' || b == b'=';
        if !is_valid_body {
            break;
        }
        body_end += 1;
    }
    if body_end > body_start {
        Some(&stream[body_start..body_end])
    } else {
        None
    }
}

#[test]
fn echo_passthrough() {
    let output = Command::new(mock_bin())
        .arg("echo")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.as_mut().unwrap().write_all(b"hello world")?;
            drop(child.stdin.take());
            child.wait_with_output()
        })
        .expect("failed to run");
    assert_eq!(output.stdout, b"hello world");
}

#[test]
fn echo_empty_input() {
    let output = Command::new(mock_bin())
        .arg("echo")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            drop(child.stdin.take());
            child.wait_with_output()
        })
        .expect("failed to run");
    assert_eq!(output.stdout, b"");
}

#[test]
fn sleep_exits_after_duration() {
    let start = std::time::Instant::now();
    let output = Command::new(mock_bin())
        .args(["sleep", "50"])
        .output()
        .expect("failed to run");
    let elapsed = start.elapsed();
    assert!(output.status.success());
    assert!(
        elapsed >= Duration::from_millis(40),
        "sleep should take at least 40ms, took {elapsed:?}"
    );
}

#[test]
fn sleep_default_duration() {
    let start = std::time::Instant::now();
    let output = Command::new(mock_bin())
        .arg("sleep")
        .output()
        .expect("failed to run");
    let elapsed = start.elapsed();
    assert!(output.status.success());
    assert!(
        elapsed >= Duration::from_millis(900),
        "default sleep should take ~1000ms, took {elapsed:?}"
    );
}

#[test]
fn exit_code_forwarded() {
    let output = Command::new(mock_bin())
        .args(["exit", "42"])
        .output()
        .expect("failed to run");
    assert_eq!(output.status.code(), Some(42));
}

#[test]
fn exit_default_code() {
    let output = Command::new(mock_bin())
        .arg("exit")
        .output()
        .expect("failed to run");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn unknown_subcommand_fails() {
    let output = Command::new(mock_bin())
        .arg("bogus")
        .output()
        .expect("failed to run");
    assert!(!output.status.success());
}

#[test]
fn no_args_fails() {
    let output = Command::new(mock_bin()).output().expect("failed to run");
    assert!(!output.status.success());
}

#[test]
fn osc52_writes_clipboard_sequence() {
    let output = Command::new(mock_bin())
        .arg("osc52")
        .output()
        .expect("failed to run");
    assert!(output.status.success());
    let payload = find_osc52_payload(&output.stdout);
    assert_eq!(
        payload,
        Some(&b"c;dGVzdA=="[..]),
        "osc52 subcommand should write OSC 52 payload to stdout, got: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}
