use std::io::{self, Read, Write};
use std::time::Duration;

use term_session_mock::{CHECK_PID_ALIVE, CHECK_PID_DEAD, process_is_alive};
use term_session_muxio_service_definitions::path_wire;

/// Deterministic mock binary for session server E2E tests.
///
/// Subcommands:
/// - `echo` — reads stdin, writes to stdout (unbuffered pass-through).
///   On Windows, enables raw VT mode so ANSI escape sequences
///   pass through ConPTY without being consumed as INPUT_RECORDs.
/// - `osc52` — writes a pre-defined OSC 52 clipboard sequence to stdout,
///   sleeps 500 ms, then exits. Lets E2E tests exercise the server's
///   session-exit output retention path. On Windows, temporarily disables VT
///   processing so the ESC byte isn't intercepted by ConPTY.
/// - `osc52_alive` — writes the same OSC 52 sequence to stdout, then stays
///   alive (echoes stdin until EOF) so tests can subscribe without racing
///   the process exiting.
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
/// - `envvar <NAME> <file>` — writes `<NAME>=<value>` (the value of the given
///   environment variable, empty if unset) to the given (absolute) file path,
///   then exits. Lets E2E tests verify which env vars reach a session child.
pub const OSC52_TEST_PAYLOAD: &[u8] = b"c;dGVzdA==";

#[cfg(windows)]
mod win_console {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{GetConsoleMode, SetConsoleMode};

    const ENABLE_LINE_INPUT: u32 = 0x0002;
    const ENABLE_ECHO_INPUT: u32 = 0x0004;
    const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    /// Try to set console mode on `handle` if it is a console handle.
    unsafe fn try_set_mode(handle: *mut std::ffi::c_void, f: impl FnOnce(u32) -> u32) -> bool {
        unsafe {
            let mut mode = 0u32;
            if GetConsoleMode(handle, &mut mode) != 0 {
                let new_mode = f(mode);
                SetConsoleMode(handle, new_mode);
                return true;
            }
            false
        }
    }

    /// Open `CONOUT$` / `CONIN$` as a fallback when std handles are pipes
    /// (the case under ConPTY: `stdin`/`stdout` are anonymous pipes, not
    /// console handles, so `GetConsoleMode` on them fails). The console
    /// device is still reachable via the named device.
    unsafe fn with_con_device<F>(name: &str, f: F) -> bool
    where
        F: FnOnce(*mut std::ffi::c_void) -> bool,
    {
        // Encode as UTF-16 NUL-terminated.
        let mut wide: Vec<u16> = name.encode_utf16().collect();
        wide.push(0);
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0x80000000 | 0x40000000, // GENERIC_READ|GENERIC_WRITE
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle.is_null() || handle as isize == -1 {
            return false;
        }
        let ok = f(handle);
        // SAFETY: handle was opened by CreateFileW
        unsafe { CloseHandle(handle) };
        ok
    }

    pub fn enable_raw_vt() {
        unsafe {
            // Stdin: try std handle, then CONIN$
            let stdin_handle = std::io::stdin().as_raw_handle();
            if !try_set_mode(stdin_handle, |m| {
                (m & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT))
                    | ENABLE_VIRTUAL_TERMINAL_INPUT
            }) {
                with_con_device("CONIN$", |h| {
                    try_set_mode(h, |m| {
                        (m & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT))
                            | ENABLE_VIRTUAL_TERMINAL_INPUT
                    });
                    true
                });
            }
            // Stdout: try std handle, then CONOUT$
            let stdout_handle = std::io::stdout().as_raw_handle();
            if !try_set_mode(stdout_handle, |m| m | ENABLE_VIRTUAL_TERMINAL_PROCESSING) {
                with_con_device("CONOUT$", |h| {
                    try_set_mode(h, |m| m | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
                    true
                });
            }
        }
    }

    pub fn disable_stdout_vt_processing() {
        unsafe {
            let stdout_handle = std::io::stdout().as_raw_handle();
            if try_set_mode(stdout_handle, |m| m & !ENABLE_VIRTUAL_TERMINAL_PROCESSING) {
                return;
            }
            // Fallback: CONOUT$ is the real console device under ConPTY
            with_con_device("CONOUT$", |h| {
                try_set_mode(h, |m| m & !ENABLE_VIRTUAL_TERMINAL_PROCESSING);
                true
            });
        }
    }
}

/// Disable kernel PTY line-discipline echo on stdin so input writes do not
/// duplicate into the master read buffer or interleave with stdout.
///
/// Without this, running a stdin-echoing mock (`echo`, `spawn_child`) inside a
/// canonical-mode PTY produces two concurrent output streams on the master: the
/// kernel's echo of the input AND the mock's own stdout echo. Under slow or
/// coverage-instrumented timing they interleave mid-marker, fragmenting byte
/// sequences (e.g. `m057` split across the two streams) and making
/// burst-ordering assertions timing-dependent.
fn disable_stdin_echo() {
    #[cfg(unix)]
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(libc::STDIN_FILENO, &mut termios) == 0 {
            termios.c_lflag &= !(libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ECHONL);
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &termios);
        }
    }

    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        use windows_sys::Win32::System::Console::{
            ENABLE_ECHO_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
        };
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode: u32 = 0;
        let mut done = false;
        if GetConsoleMode(handle, &mut mode) != 0 {
            let _ = SetConsoleMode(handle, mode & !ENABLE_ECHO_INPUT);
            done = true;
        }
        if !done {
            // Fallback to CONIN$ when stdin is a pipe (ConPTY)
            let mut wide: Vec<u16> = "CONIN$".encode_utf16().collect();
            wide.push(0);
            let conin = CreateFileW(
                wide.as_ptr(),
                0x80000000 | 0x40000000,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if !conin.is_null() && conin as isize != -1 {
                if GetConsoleMode(conin, &mut mode) != 0 {
                    SetConsoleMode(conin, mode & !ENABLE_ECHO_INPUT);
                }
                CloseHandle(conin);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Usage: term_session_mock <echo|osc52|osc52_alive|sleep|exit|spawn_child|check_pid|pwd|envvar> [args]"
        );
        std::process::exit(1);
    }

    match args[1].as_str() {
        "echo" => {
            #[cfg(windows)]
            win_console::enable_raw_vt();
            disable_stdin_echo();

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
        "osc52_alive" => {
            #[cfg(windows)]
            win_console::disable_stdout_vt_processing();
            disable_stdin_echo();

            let mut stdout = io::stdout();
            let _ = stdout.write_all(b"\x1b]52;");
            let _ = stdout.write_all(OSC52_TEST_PAYLOAD);
            let _ = stdout.write_all(b"\x07");
            let _ = stdout.flush();
            // Brief yield so ConPTY flushes the console buffer to the
            // PTY master pipe before we block on stdin. Without this,
            // the 10-byte OSC may stay buffered (no newline, not exiting)
            // and the subscriber's wait_for_output times out.
            std::thread::sleep(Duration::from_millis(50));

            // Stay alive until the session is killed (stdin EOF), so tests
            // can subscribe without racing this process exiting.
            let mut buffer = [0u8; 4096];
            let mut stdin = io::stdin();
            loop {
                match stdin.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
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
            disable_stdin_echo();

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
            let file = std::env::args_os()
                .nth(2)
                .expect("pwd requires an absolute output file path");
            // Report the child's actual cwd as raw wire bytes (lossless, see
            // `path_wire`), so the harness can assert a byte-for-byte round-trip
            // even for non-UTF-8 paths.
            let cwd = std::env::current_dir()
                .map(|p| path_wire::encode_path(&p))
                .unwrap_or_default();
            if let Err(e) = std::fs::write(&file, &cwd) {
                eprintln!("pwd: failed to write {:?}: {e}", file);
                std::process::exit(1);
            }
        }
        "envvar" => {
            // Write `<NAME>=<value>` for the named environment variable (value
            // empty when unset) to an absolute file path, mirroring `pwd`. Lets
            // E2E tests verify which env vars reach a session child.
            let name = std::env::args_os()
                .nth(2)
                .expect("envvar requires an env var name");
            let file = std::env::args_os()
                .nth(3)
                .expect("envvar requires an absolute output file path");
            let value = std::env::var_os(&name).unwrap_or_default();
            let mut report = name.as_encoded_bytes().to_vec();
            report.push(b'=');
            report.extend_from_slice(value.as_encoded_bytes());
            if let Err(e) = std::fs::write(&file, &report) {
                eprintln!("envvar: failed to write {:?}: {e}", file);
                std::process::exit(1);
            }
        }
        other => {
            eprintln!("Unknown subcommand: {other}");
            std::process::exit(1);
        }
    }
}
