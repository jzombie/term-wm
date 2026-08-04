use std::io::{self, Read, Write};
use std::time::Duration;

use term_session_mock::{CHECK_PID_ALIVE, CHECK_PID_DEAD, process_is_alive};

/// Deterministic mock binary for session server E2E tests.
///
/// Subcommands:
/// - `echo` — reads stdin, writes to stdout (unbuffered pass-through).
///   On Windows, enables raw VT mode so ANSI escape sequences
///   pass through ConPTY without being consumed as INPUT_RECORDs.
/// - `osc52` — writes a pre-defined OSC 52 clipboard sequence to stdout.
///   On Windows, temporarily disables VT processing so the ESC
///   byte isn't intercepted by ConPTY.
/// - `sleep <ms>` — sleeps for N milliseconds, then exits.
/// - `exit <code>` — exits with the given status code.
/// - `spawn_child <ms>` — spawns a grandchild (`sleep <ms>`), prints
///   `GRANDCHILD_PID:<pid>` to stdout, then echoes stdin until EOF.
///   Used to prove that kill paths tear down the whole tree (grandchildren
///   included), not just the session leader.
/// - `check_pid <pid>` — exits 0 if the process is alive, non-zero otherwise.
///   Cross-platform liveness probe for tree-kill assertions.
/// - `pwd <file>` — writes the process's current working directory to the given
///   (absolute) file path, then exits. Lets E2E tests verify session cwd.
pub const OSC52_TEST_PAYLOAD: &[u8] = b"c;dGVzdA==";

#[cfg(windows)]
mod win_console {
    use std::os::windows::io::AsRawHandle;

    unsafe extern "system" {
        fn GetConsoleMode(handle: *mut std::ffi::c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut std::ffi::c_void, mode: u32) -> i32;
    }

    const ENABLE_LINE_INPUT: u32 = 0x0002;
    const ENABLE_ECHO_INPUT: u32 = 0x0004;
    const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    pub fn enable_raw_vt() {
        unsafe {
            let stdin_handle = std::io::stdin().as_raw_handle();
            let mut mode = 0u32;
            if GetConsoleMode(stdin_handle, &mut mode) != 0 {
                mode &= !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT);
                mode |= ENABLE_VIRTUAL_TERMINAL_INPUT;
                SetConsoleMode(stdin_handle, mode);
            }
            let stdout_handle = std::io::stdout().as_raw_handle();
            if GetConsoleMode(stdout_handle, &mut mode) != 0 {
                mode |= ENABLE_VIRTUAL_TERMINAL_PROCESSING;
                SetConsoleMode(stdout_handle, mode);
            }
        }
    }

    pub fn disable_stdout_vt_processing() {
        unsafe {
            let stdout_handle = std::io::stdout().as_raw_handle();
            let mut mode = 0u32;
            if GetConsoleMode(stdout_handle, &mut mode) != 0 {
                mode &= !ENABLE_VIRTUAL_TERMINAL_PROCESSING;
                SetConsoleMode(stdout_handle, mode);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Usage: term_session_mock <echo|osc52|sleep|exit|spawn_child|check_pid|pwd> [args]"
        );
        std::process::exit(1);
    }

    match args[1].as_str() {
        "echo" => {
            #[cfg(windows)]
            win_console::enable_raw_vt();

            let mut buffer = [0u8; 4096];
            let mut stdin = io::stdin();
            let mut stdout = io::stdout();
            loop {
                match stdin.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        if stdout.write_all(&buffer[..n]).is_err() {
                            break;
                        }
                        if stdout.flush().is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        "osc52" => {
            #[cfg(windows)]
            win_console::disable_stdout_vt_processing();

            let mut stdout = io::stdout();
            let _ = stdout.write_all(b"\x1b]52;");
            let _ = stdout.write_all(OSC52_TEST_PAYLOAD);
            let _ = stdout.write_all(b"\x07");
            let _ = stdout.flush();
            std::thread::sleep(Duration::from_millis(500));
        }
        "capture" => {
            #[cfg(windows)]
            win_console::enable_raw_vt();

            let mut stdin = io::stdin();
            let mut stdout = io::stdout();
            let mut buf = [0u8; 1024];

            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = stdout.write_all(b"MOUSE_OK:");
                        let _ = stdout.write_all(&buf[..n]);
                        let _ = stdout.flush();
                        if buf[..n].windows(4).any(|w| w == b"ping") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        "sleep" => {
            let ms: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1000);
            std::thread::sleep(Duration::from_millis(ms));
        }
        "spawn_child" => {
            #[cfg(windows)]
            win_console::enable_raw_vt();

            let ms: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(60000);
            // Spawn a grandchild process that outlives this one unless the
            // whole tree is torn down. `sleep` keeps the process alive without
            // producing output.
            let exe = std::env::current_exe().expect("current exe");
            let mut grandchild = std::process::Command::new(exe)
                .arg("sleep")
                .arg(ms.to_string())
                .spawn()
                .expect("spawn grandchild");
            let pid = grandchild.id();

            let mut stdout = io::stdout();
            let _ = stdout.write_all(format!("GRANDCHILD_PID:{pid}\n").as_bytes());
            let _ = stdout.flush();

            // Keep this process alive (like `echo`) so the session stays up.
            let mut buffer = [0u8; 4096];
            let mut stdin = io::stdin();
            loop {
                match stdin.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        if stdout.write_all(&buffer[..n]).is_err() {
                            break;
                        }
                        if stdout.flush().is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }

            // The session kill path tears the whole tree down via the Job
            // Object / process group, but if we exit normally (stdin EOF)
            // the grandchild would otherwise be orphaned — reap it.
            let _ = grandchild.kill();
            let _ = grandchild.wait();
        }
        "check_pid" => {
            let pid: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            if process_is_alive(pid) {
                std::process::exit(CHECK_PID_ALIVE);
            }
            std::process::exit(CHECK_PID_DEAD);
        }
        "exit" => {
            let code: i32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            std::process::exit(code);
        }
        "pwd" => {
            // Write the child process's actual working directory to an absolute
            // file path (the session may be running in a different cwd than the
            // test harness, so the output file must be given as an absolute
            // path). Lets E2E tests verify where the daemon spawned the session.
            let file = args
                .get(2)
                .expect("pwd requires an absolute output file path");
            // Lossy conversion (U+FFFD substitution) keeps the report directly
            // comparable to the test harness's own `to_string_lossy`
            // canonicalization; raw bytes would need OsStrExt and aren't worth
            // it for a test probe.
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "<unavailable>".to_string());
            if let Err(e) = std::fs::write(file, &cwd) {
                eprintln!("pwd: failed to write {file}: {e}");
                std::process::exit(1);
            }
        }
        other => {
            eprintln!("Unknown subcommand: {other}");
            std::process::exit(1);
        }
    }
}
