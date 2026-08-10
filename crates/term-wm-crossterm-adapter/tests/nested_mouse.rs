//! Nested-mouse regression test.
//!
//! Guards the exact contract that makes nested mouse work: a child process
//! running inside a real PTY pair (ConPTY on Windows, Unix PTY elsewhere) must
//! be able to (1) enable mouse capture such that `ENABLE_MOUSE_INPUT` is set on
//! its *own* console input handle, and (2) receive a mouse event when the host
//! writes an SGR sequence to the PTY master. This runs headlessly on CI because
//! it spawns a programmatic PTY pair (`portable_pty::openpty`) instead of
//! relying on the test process's own (piped) stdin.
//!
//! The probe binary (`term-wm-crossterm-adapter/src/bin/mouse_test_probe.rs`)
//! runs inside the PTY pair and reports markers on its stdout:
//!   MOUSE_READY        — mouse capture enabled
//!   CONSOLE_MODE:<hex> — Windows: GetConsoleMode of the child's stdin
//!   MOUSE_EVENT:<dbg>  — a crossterm mouse event was received
//!   MOUSE_TIMEOUT      — poll window elapsed with no event
//!   MOUSE_ENABLE_ERROR — set_mouse_capture failed
//!
//! The test also plays the *host* role: it answers the child's DSR cursor
//! query (`\x1b[6n` → `\x1b[1;1R`), which a Windows console child stalls on
//! until answered.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

#[cfg(windows)]
const MOUSE_INPUT_FLAGS: u32 = 0x0010 | 0x0080 | 0x0008;
#[cfg(windows)]
const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

/// Accumulate probe output until `needle` appears, panicking with the output
/// seen so far on timeout / disconnect.
fn wait_for_output(
    rx: &std::sync::mpsc::Receiver<Vec<u8>>,
    accumulated: &mut Vec<u8>,
    needle: &str,
    deadline: Instant,
    label: &str,
) {
    let needle = needle.as_bytes();
    while !accumulated.windows(needle.len()).any(|w| w == needle) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(chunk) => accumulated.extend_from_slice(&chunk),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!(
                "timed out waiting for {label}; probe output so far:\n{}",
                String::from_utf8_lossy(accumulated)
            ),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!(
                "probe exited before {label}; output so far:\n{}",
                String::from_utf8_lossy(accumulated)
            ),
        }
    }
}

#[test]
fn nested_mouse_child_receives_routed_mouse() {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let mut child = pair
        .slave
        .spawn_command(CommandBuilder::new(env!("CARGO_BIN_EXE_mouse_test_probe")))
        .expect("spawn mouse probe");
    drop(pair.slave);

    // The writer is shared: the host loop answers DSR queries, and the test
    // later writes the routed SGR mouse input through it.
    let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(
        pair.master.take_writer().expect("take master writer"),
    ));
    let writer_for_host = Arc::clone(&writer);

    // Pump the probe's stdout; answer the ConPTY/child DSR startup handshake
    // (`\x1b[6n` → `\x1b[1;1R`) so the child doesn't stall before its first
    // report (the host role a real emulator plays).
    let (tx, rx) = std::sync::mpsc::channel();
    let mut reader = pair.master.try_clone_reader().expect("clone master reader");
    let _host_thread = std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        let mut pending: Vec<u8> = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    // Answer each outstanding DSR cursor-position query.
                    while let Some(pos) = pending.windows(4).position(|w| w == b"\x1b[6n") {
                        pending.drain(..pos + 4);
                        if let Ok(mut w) = writer_for_host.lock() {
                            let _ = w.write_all(b"\x1b[1;1R");
                            let _ = w.flush();
                        }
                    }
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let start = Instant::now();
    let deadline = start + Duration::from_secs(20);
    let mut out = Vec::new();

    wait_for_output(&rx, &mut out, "MOUSE_READY", deadline, "MOUSE_READY");

    // Windows: nested mouse only works when `set_mouse_capture` *replaces* the
    // child's console input mode with the mouse flags (0x98), clearing the
    // ENABLE_VIRTUAL_TERMINAL_INPUT the probe started with. The OR/AND variant
    // preserves 0x200 (→ 0x298) and the host's routed SGR never surfaces as a
    // MOUSE_EVENT_RECORD. Assert the precondition (started at 0x200) and the
    // exact working mode; the MOUSE_EVENT check below is the end-to-end verdict.
    #[cfg(windows)]
    {
        wait_for_output(
            &rx,
            &mut out,
            "CONSOLE_MODE_BEFORE:",
            deadline,
            "CONSOLE_MODE_BEFORE",
        );
        wait_for_output(&rx, &mut out, "CONSOLE_MODE:", deadline, "CONSOLE_MODE");

        fn parse_mode(out: &[u8], marker: &str) -> u32 {
            let text = String::from_utf8_lossy(out);
            let line = text
                .split('\n')
                .find(|l| l.contains(marker))
                .unwrap_or_else(|| panic!("{marker} marker present"));
            let hex = line.trim().rsplit(':').next().unwrap_or("");
            u32::from_str_radix(hex.trim_start_matches("0x"), 16)
                .unwrap_or_else(|_| panic!("parse {marker} hex value"))
        }

        let before = parse_mode(&out, "CONSOLE_MODE_BEFORE:");
        let after = parse_mode(&out, "CONSOLE_MODE:");
        assert_eq!(
            before & ENABLE_VIRTUAL_TERMINAL_INPUT,
            ENABLE_VIRTUAL_TERMINAL_INPUT,
            "probe must start with ENABLE_VIRTUAL_TERMINAL_INPUT set (like term-wm's \
             raw-mode state) or the test can't discriminate; before={before:#010x}"
        );
        assert_eq!(
            after, MOUSE_INPUT_FLAGS,
            "set_mouse_capture must replace the mode with exactly the mouse flags \
             (0x{MOUSE_INPUT_FLAGS:x}), clearing ENABLE_VIRTUAL_TERMINAL_INPUT; \
             before={before:#010x} after={after:#010x}"
        );
    }

    // Simulate the host emulator routing mouse to the child: SGR left-button
    // press at column 5, row 10, written to the PTY master input.
    {
        let mut w = writer.lock().expect("lock master writer");
        w.write_all(b"\x1b[<0;5;10M")
            .expect("write SGR mouse to PTY master");
        w.flush().expect("flush SGR mouse");
    }

    // The child must actually receive the routed mouse event.
    wait_for_output(&rx, &mut out, "MOUSE_EVENT", deadline, "MOUSE_EVENT");
    let text = String::from_utf8_lossy(&out);
    let event_line = text
        .split('\n')
        .find(|line| line.contains("MOUSE_EVENT:"))
        .expect("MOUSE_EVENT marker present");
    assert!(
        event_line.contains("column"),
        "child must receive a routed mouse event with coordinates; got: {event_line}"
    );

    let _ = child.kill();
    drop(child);
}
