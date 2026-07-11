use std::io::{self, Read, Write};
use std::time::Duration;

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
        eprintln!("Usage: term_session_mock <echo|osc52|sleep|exit> [args]");
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
        "sleep" => {
            let ms: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1000);
            std::thread::sleep(Duration::from_millis(ms));
        }
        "exit" => {
            let code: i32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            std::process::exit(code);
        }
        other => {
            eprintln!("Unknown subcommand: {other}");
            std::process::exit(1);
        }
    }
}
